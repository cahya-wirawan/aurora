//! Tests of the selection refinements (0.216.0): brute-force references
//! for the distance transform, the dilation and the blur, identities,
//! inverted selections, caps, tight bounds and determinism.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::float_cmp,
    clippy::unwrap_used,
    clippy::panic,
    clippy::many_single_char_names,
    clippy::redundant_closure_for_method_calls
)]

use std::sync::Arc;

use aurora_core::Rect;

use crate::selection_refine::{MAX_REFINE_WORK, disk_rows, feather_box_radii, nearest_sites};
use crate::{
    DocError, MAX_FEATHER_PX, MAX_REFINE_PX, MAX_SELECTION_MASK_PIXELS, MAX_SMOOTH_PX, Selection,
    SelectionMask, SelectionShape, border, contract, expand, feather, smooth,
};

fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn mask_selection(bounds: Rect, data: Vec<u8>) -> Selection {
    Selection::from_mask(Arc::new(SelectionMask::new(bounds, data).unwrap()))
}

/// A random `w x h` mask at `(x, y)` with at least one texel in the region;
/// `soft` mixes in partial coverage, otherwise it is binary.
fn random_mask(rng: &mut Lcg, x: i64, y: i64, w: u32, h: u32, soft: bool) -> Selection {
    loop {
        let data: Vec<u8> = (0..w * h)
            .map(|_| match rng.below(3) {
                0 => 0,
                1 => 255,
                _ => {
                    if soft {
                        rng.below(256) as u8
                    } else {
                        255
                    }
                }
            })
            .collect();
        if data.iter().any(|&b| b >= 128) {
            return mask_selection(rect(x, y, w, h), data);
        }
    }
}

/// The effective coverage byte at `(x, y)`.
fn byte(sel: &Selection, x: i64, y: i64) -> u8 {
    (sel.coverage_at(x, y) * 255.0).round() as u8
}

/// The effective coverage over `window`, row-major.
fn bytes(sel: &Selection, window: &Rect) -> Vec<u8> {
    let mut out = Vec::new();
    for y in window.y..window.bottom() {
        for x in window.x..window.right() {
            out.push(byte(sel, x, y));
        }
    }
    out
}

fn solid(r: Rect) -> Selection {
    Selection::new(r)
}

// ---- the distance transform ----

#[test]
fn the_distance_transform_matches_a_brute_force_search() {
    let mut rng = Lcg(7);
    for case in 0..60 {
        let (w, h) = (2 + rng.below(9) as usize, 2 + rng.below(9) as usize);
        let pad = rng.below(4) as usize;
        let outside = case % 3 == 0;
        let cap = 50;
        let src: Vec<u8> = (0..w * h)
            .map(|_| {
                if rng.below(4) == 0 {
                    (128 + rng.below(128)) as u8
                } else {
                    rng.below(128) as u8
                }
            })
            .collect();
        let (d2, cc) = nearest_sites(&src, w, h, pad, outside, cap);
        let (ow, oh) = (w + 2 * pad, h + 2 * pad);
        for yo in 0..oh {
            for xo in 0..ow {
                // Every nearest site's squared distance and coverage.
                let mut best = u32::MAX;
                let mut covers: Vec<u8> = Vec::new();
                let mut consider = |sx: i64, sy: i64, c: u8| {
                    let d = ((xo as i64 - sx).pow(2) + (yo as i64 - sy).pow(2)) as u32;
                    if d < best {
                        best = d;
                        covers.clear();
                    }
                    if d == best {
                        covers.push(c);
                    }
                };
                for sy in 0..h {
                    for sx in 0..w {
                        let b = src[sy * w + sx];
                        if b >= 128 {
                            consider((sx + pad) as i64, (sy + pad) as i64, b);
                        }
                    }
                }
                if outside {
                    if xo < pad || xo >= pad + w || yo < pad || yo >= pad + h {
                        consider(xo as i64, yo as i64, 255);
                    }
                    // The nearest outside texel is on one of four lines.
                    for (sx, sy) in [
                        (pad as i64 - 1, yo as i64),
                        ((pad + w) as i64, yo as i64),
                        (xo as i64, pad as i64 - 1),
                        (xo as i64, (pad + h) as i64),
                    ] {
                        consider(sx, sy, 255);
                    }
                }
                let i = yo * ow + xo;
                assert_eq!(d2[i], best.min(cap), "case {case} at ({xo}, {yo})");
                if best < cap {
                    assert!(covers.contains(&cc[i]), "case {case} at ({xo}, {yo})");
                }
            }
        }
    }
}

// ---- Expand and Contract ----

/// The brute-force dilation of the `255` texels of a binary mask.
fn reference_expand(sel: &Selection, px: u32) -> Vec<(i64, i64, u8)> {
    let b = sel.bounds;
    let mut sites = Vec::new();
    for y in b.y..b.bottom() {
        for x in b.x..b.right() {
            if byte(sel, x, y) == 255 {
                sites.push((x, y));
            }
        }
    }
    let r = i64::from(px);
    let mut out = Vec::new();
    for y in b.y - r..b.bottom() + r {
        for x in b.x - r..b.right() + r {
            let own = byte(sel, x, y);
            let d2 = sites
                .iter()
                .map(|&(sx, sy)| (x - sx).pow(2) + (y - sy).pow(2))
                .min()
                .unwrap_or(i64::MAX);
            let cov = if d2 > (r + 1) * (r + 1) {
                0.0
            } else {
                (px as f32 + 1.0 - (d2 as f32).sqrt()).clamp(0.0, 1.0)
            };
            out.push((x, y, ((cov * 255.0).round() as u8).max(own)));
        }
    }
    out
}

#[test]
fn expand_matches_the_brute_force_dilation_of_binary_masks() {
    let mut rng = Lcg(11);
    for case in 0..40 {
        let (w, h) = (1 + rng.below(10) as u32, 1 + rng.below(10) as u32);
        let sel = random_mask(&mut rng, 3, -2, w, h, false);
        let px = 1 + rng.below(5) as u32;
        let got = expand(&sel, px).unwrap().unwrap();
        for (x, y, want) in reference_expand(&sel, px) {
            assert_eq!(byte(&got, x, y), want, "case {case} px {px} at ({x}, {y})");
        }
    }
}

#[test]
fn expand_grows_a_rectangle_by_exactly_its_distance() {
    let got = expand(&solid(rect(10, 10, 6, 4)), 3).unwrap().unwrap();
    assert_eq!(got.bounds, rect(7, 7, 12, 10));
    // The straight sides are full for the whole distance and zero past it.
    assert_eq!(byte(&got, 12, 7), 255);
    assert_eq!(byte(&got, 12, 6), 0);
    assert_eq!(byte(&got, 7, 12), 255);
    assert_eq!(byte(&got, 6, 12), 0);
    // The corner is rounded (a disk, not a square).
    assert!(byte(&got, 7, 7) < 128);
    assert_eq!(byte(&got, 9, 9), 255);
}

#[test]
fn contract_shrinks_a_rectangle_and_a_thin_one_vanishes() {
    let got = contract(&solid(rect(10, 10, 8, 6)), 2).unwrap().unwrap();
    assert_eq!(got.bounds, rect(12, 12, 4, 2));
    assert!(matches!(got.shape, SelectionShape::Rect));
    assert!(contract(&solid(rect(0, 0, 10, 4)), 2).unwrap().is_none());
    assert!(contract(&solid(rect(0, 0, 3, 100)), 2).unwrap().is_none());
}

#[test]
fn expand_then_contract_of_convex_shapes_is_close_to_the_identity() {
    for sel in [
        solid(rect(5, 5, 20, 14)),
        Selection::ellipse(rect(0, 0, 31, 23)),
    ] {
        for px in [1_u32, 3, 6] {
            let back = contract(&expand(&sel, px).unwrap().unwrap(), px)
                .unwrap()
                .unwrap();
            let window = rect(-10, -10, 60, 50);
            let (a, b) = (bytes(&sel, &window), bytes(&back, &window));
            let off = a
                .iter()
                .zip(&b)
                .filter(|(p, q)| p.abs_diff(**q) > 64)
                .count();
            // Only a few edge texels of the ellipse differ noticeably.
            assert!(off <= 24, "px {px}: {off} texels differ");
        }
    }
    // A rectangle comes back exactly.
    let rectangle = solid(rect(5, 5, 20, 14));
    let back = contract(&expand(&rectangle, 4).unwrap().unwrap(), 4)
        .unwrap()
        .unwrap();
    assert_eq!(back, rectangle);
}

#[test]
fn expand_never_lowers_and_contract_never_raises() {
    let mut rng = Lcg(21);
    for _ in 0..30 {
        let sel = random_mask(&mut rng, 0, 0, 12, 9, true);
        let window = rect(-6, -6, 24, 21);
        let base = bytes(&sel, &window);
        let up = expand(&sel, 2).unwrap().unwrap();
        for (a, b) in base.iter().zip(bytes(&up, &window)) {
            assert!(b >= *a);
        }
        if let Some(down) = contract(&sel, 2).unwrap() {
            for (a, b) in base.iter().zip(bytes(&down, &window)) {
                assert!(b <= *a);
            }
        }
    }
}

#[test]
fn a_zero_distance_returns_an_equal_selection() {
    let sel = Selection::ellipse(rect(2, 3, 9, 7));
    assert_eq!(expand(&sel, 0).unwrap(), Some(sel.clone()));
    assert_eq!(contract(&sel, 0).unwrap(), Some(sel.clone()));
    assert_eq!(smooth(&sel, 0).unwrap(), Some(sel.clone()));
    assert_eq!(feather(&sel, 0.0).unwrap(), Some(sel.clone()));
    assert_eq!(feather(&sel, 0.4).unwrap(), Some(sel));
}

// ---- inverted selections ----

/// `refined` is, texel for texel over `window`, the complement of `dual`.
fn assert_complement(refined: &Selection, dual: Option<&Selection>, window: &Rect) {
    for y in window.y..window.bottom() {
        for x in window.x..window.right() {
            let d = dual.map_or(0, |d| byte(d, x, y));
            assert_eq!(byte(refined, x, y), 255 - d, "at ({x}, {y})");
        }
    }
}

#[test]
fn an_inverted_expand_is_the_complement_of_the_contract_and_the_reverse() {
    let window = rect(-12, -12, 50, 44);
    let mut rng = Lcg(31);
    for soft in [false, true] {
        let shape = random_mask(&mut rng, 4, 3, 14, 11, soft);
        let mut inv = shape.clone();
        inv.inverted = true;
        let e = expand(&inv, 2).unwrap().unwrap();
        assert!(e.inverted);
        assert_complement(&e, contract(&shape, 2).unwrap().as_ref(), &window);
        let c = contract(&inv, 2).unwrap().unwrap();
        assert!(c.inverted);
        assert_complement(&c, expand(&shape, 2).unwrap().as_ref(), &window);
        let f = feather(&inv, 4.0).unwrap().unwrap();
        assert!(f.inverted);
        assert_complement(&f, feather(&shape, 4.0).unwrap().as_ref(), &window);
        let s = smooth(&inv, 2).unwrap().unwrap();
        assert!(s.inverted);
        assert_complement(&s, smooth(&shape, 2).unwrap().as_ref(), &window);
    }
}

#[test]
fn an_inverted_hole_shrinks_on_expand_and_grows_on_contract() {
    let mut inv = solid(rect(10, 10, 6, 6));
    inv.inverted = true;
    // Expanding the inverted selection erodes the hole away entirely, so
    // everything is selected.
    let e = expand(&inv, 3).unwrap().unwrap();
    assert!(e.inverted);
    assert!(e.bounds.is_empty());
    assert_eq!(byte(&e, 12, 12), 255);
    // Contracting it grows the hole.
    let c = contract(&inv, 2).unwrap().unwrap();
    assert!(c.inverted);
    assert_eq!(byte(&c, 9, 9), 0);
    assert_eq!(byte(&c, 8, 12), 0);
    assert_eq!(byte(&c, 7, 12), 255);
}

#[test]
fn everything_is_unchanged_by_all_but_border() {
    let all = Selection {
        bounds: rect(0, 0, 0, 0),
        inverted: true,
        shape: SelectionShape::Rect,
    };
    assert_eq!(expand(&all, 4).unwrap(), Some(all.clone()));
    assert_eq!(contract(&all, 4).unwrap(), Some(all.clone()));
    assert_eq!(smooth(&all, 4).unwrap(), Some(all.clone()));
    assert_eq!(feather(&all, 4.0).unwrap(), Some(all.clone()));
    assert!(border(&all, 4).unwrap().is_none());
    let nothing = solid(rect(0, 0, 0, 0));
    assert!(expand(&nothing, 4).unwrap().is_none());
}

// ---- Feather ----

/// The composite one-dimensional kernel of the three box blurs.
fn composite_kernel(radii: [u32; 3]) -> Vec<f64> {
    let mut kernel = vec![1.0_f64];
    for k in radii {
        let n = (2 * k + 1) as usize;
        let mut next = vec![0.0; kernel.len() + n - 1];
        for (i, &a) in kernel.iter().enumerate() {
            for j in 0..n {
                next[i + j] += a / n as f64;
            }
        }
        kernel = next;
    }
    kernel
}

#[test]
fn feather_matches_a_direct_convolution_with_the_composite_kernel() {
    let mut rng = Lcg(41);
    for radius in [1.0_f32, 2.5, 6.0, 11.0] {
        let sel = random_mask(&mut rng, 5, 4, 13, 10, true);
        let radii = feather_box_radii(radius).unwrap();
        let kernel = composite_kernel(radii);
        let half = (kernel.len() / 2) as i64;
        let got = feather(&sel, radius).unwrap().unwrap();
        let b = sel.bounds;
        for y in b.y - half..b.bottom() + half {
            for x in b.x - half..b.right() + half {
                let mut sum = 0.0;
                for (j, ky) in kernel.iter().enumerate() {
                    for (i, kx) in kernel.iter().enumerate() {
                        sum += ky
                            * kx
                            * f64::from(byte(&sel, x + i as i64 - half, y + j as i64 - half));
                    }
                }
                let want = sum.round() as i64;
                let have = i64::from(byte(&got, x, y));
                assert!(
                    (want - have).abs() <= 1,
                    "radius {radius} at ({x}, {y}): {have} vs {want}"
                );
            }
        }
    }
}

#[test]
fn the_box_passes_have_the_variance_of_the_gaussian() {
    for radius in [1.0_f32, 2.0, 3.0, 5.0, 8.0, 16.0, 40.0, 100.0, 250.0] {
        let radii = feather_box_radii(radius).unwrap();
        let kernel = composite_kernel(radii);
        let half = (kernel.len() / 2) as f64;
        let var: f64 = kernel
            .iter()
            .enumerate()
            .map(|(i, &p)| p * (i as f64 - half).powi(2))
            .sum();
        let sigma = f64::from(radius) / 2.0;
        // The widths are odd integers, so the match is to the best
        // achievable: within 8 % or 0.55 of a pixel.
        assert!(
            (var.sqrt() - sigma).abs() <= (sigma * 0.08).max(0.55),
            "radius {radius}: sigma {} vs {sigma}",
            var.sqrt()
        );
        assert!(radii.iter().sum::<u32>() as f32 <= 1.5 * radius + 3.0);
        assert_eq!(kernel.len() as u32, 2 * radii.iter().sum::<u32>() + 1);
    }
    assert!(feather_box_radii(0.4).is_none());
    assert!(feather_box_radii(0.9).is_none());
}

#[test]
fn a_feathered_step_is_close_to_the_gaussian_integral() {
    // A wide rectangle's left edge: the profile across it is the
    // Gaussian's cumulative distribution.
    let radius = 12.0_f32;
    let got = feather(&solid(rect(0, 0, 200, 60)), radius)
        .unwrap()
        .unwrap();
    let sigma = f64::from(radius) / 2.0;
    let erf = |x: f64| {
        // Abramowitz and Stegun 7.1.26.
        let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
        let y = 1.0
            - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736)
                * t
                + 0.254_829_592)
                * t
                * (-x * x).exp();
        if x >= 0.0 { y } else { -y }
    };
    for dx in -20_i64..20 {
        let want = 0.5 * (1.0 + erf((dx as f64 + 0.5) / (sigma * std::f64::consts::SQRT_2)));
        let have = f64::from(byte(&got, dx, 30)) / 255.0;
        assert!((want - have).abs() < 0.04, "dx {dx}: {have} vs {want}");
    }
}

#[test]
fn feather_conserves_coverage_and_grows_within_its_radius() {
    let mut rng = Lcg(51);
    for radius in [2.0_f32, 7.0, 15.0] {
        let sel = random_mask(&mut rng, 20, 20, 16, 12, true);
        let before: u64 = bytes(&sel, &sel.bounds).iter().map(|&b| u64::from(b)).sum();
        let got = feather(&sel, radius).unwrap().unwrap();
        let after: u64 = bytes(&got, &got.bounds).iter().map(|&b| u64::from(b)).sum();
        // Only byte rounding differs.
        assert!(
            before.abs_diff(after) <= got.bounds.area_px() / 2 + 1,
            "radius {radius}: {before} vs {after}"
        );
        let reach = i64::from((1.5 * radius) as u32 + 3);
        assert!(got.bounds.x >= sel.bounds.x - reach);
        assert!(got.bounds.right() <= sel.bounds.right() + reach);
        assert!(got.bounds.x < sel.bounds.x, "the edge softens outward");
    }
}

#[test]
fn feather_of_a_rectangle_is_symmetric_and_keeps_its_centre_solid() {
    let got = feather(&solid(rect(0, 0, 40, 40)), 6.0).unwrap().unwrap();
    assert_eq!(byte(&got, 20, 20), 255);
    for d in 0..8 {
        assert_eq!(byte(&got, -d - 1, 20), byte(&got, 40 + d, 20), "d {d}");
        assert_eq!(byte(&got, 20, -d - 1), byte(&got, 20, 40 + d), "d {d}");
        assert_eq!(byte(&got, -d - 1, 20), byte(&got, 20, -d - 1), "d {d}");
    }
    let row: Vec<u8> = (-9..10).map(|x| byte(&got, x, 20)).collect();
    assert!(row.windows(2).all(|w| w[0] <= w[1]), "{row:?}");
}

// ---- Smooth ----

#[test]
fn smooth_removes_specks_fills_pinholes_and_keeps_a_straight_edge() {
    let mut data = vec![0_u8; 40 * 40];
    for y in 8..32 {
        for x in 8..32 {
            data[y * 40 + x] = 255;
        }
    }
    data[20 * 40 + 20] = 0; // a pin-hole
    data[2 * 40 + 2] = 255; // a speck
    let sel = mask_selection(rect(0, 0, 40, 40), data);
    let got = smooth(&sel, 3).unwrap().unwrap();
    assert_eq!(byte(&got, 20, 20), 255, "the pin-hole is filled");
    assert_eq!(byte(&got, 2, 2), 0, "the speck is gone");
    assert_eq!(byte(&got, 8, 20), 255);
    assert_eq!(byte(&got, 7, 20), 0);
    assert_eq!(byte(&got, 31, 20), 255);
    assert_eq!(byte(&got, 32, 20), 0);
    // The corner is rounded off.
    assert!(byte(&got, 8, 8) < 255);
}

#[test]
fn smooth_never_leaves_the_bounds_and_a_solid_rectangle_loses_only_corners() {
    let got = smooth(&solid(rect(0, 0, 30, 30)), 4).unwrap().unwrap();
    assert!(got.bounds.x >= 0 && got.bounds.right() <= 30);
    assert_eq!(byte(&got, 15, 15), 255);
    assert_eq!(byte(&got, 15, 0), 255);
    assert!(byte(&got, 0, 0) < 255);
}

#[test]
fn the_digital_disk_has_the_right_rows() {
    let (rows, count) = disk_rows(1);
    assert_eq!(rows, vec![(-1, 0), (0, 1), (1, 0)]);
    assert_eq!(count, 5);
    let (_, count) = disk_rows(2);
    assert_eq!(count, 13);
    let (_, count) = disk_rows(10);
    assert!((count as f64 - 314.0).abs() < 25.0);
}

// ---- Border ----

#[test]
fn border_is_the_ring_around_the_edge() {
    let sel = solid(rect(20, 20, 30, 30));
    for width in [1_u32, 2, 3, 6] {
        let ring = border(&sel, width).unwrap().unwrap();
        let outer = width - width / 2;
        let inner = width / 2;
        let a = expand(&sel, outer).unwrap().unwrap();
        let b = contract(&sel, inner).unwrap();
        let window = rect(10, 10, 50, 50);
        for y in window.y..window.bottom() {
            for x in window.x..window.right() {
                let eroded = b.as_ref().map_or(0, |b| byte(b, x, y));
                assert_eq!(
                    byte(&ring, x, y),
                    byte(&a, x, y).saturating_sub(eroded),
                    "width {width} at ({x}, {y})"
                );
            }
        }
        assert_eq!(byte(&ring, 35, 35), 0, "the middle is not in the ring");
        assert_eq!(byte(&ring, 35, 20 - i64::from(outer)), 255);
        assert_eq!(byte(&ring, 35, 20 - i64::from(outer) - 1), 0);
        assert!(!ring.inverted);
    }
}

#[test]
fn the_border_of_an_inverted_selection_is_a_plain_ring_with_the_sides_swapped() {
    let shape = solid(rect(20, 20, 30, 30));
    let mut inv = shape.clone();
    inv.inverted = true;
    let got = border(&inv, 3).unwrap().unwrap();
    assert!(!got.inverted);
    // Width 3 is 2 outside, 1 inside; the complement swaps them.
    let a = expand(&shape, 1).unwrap().unwrap();
    let b = contract(&shape, 2).unwrap().unwrap();
    let window = rect(10, 10, 50, 50);
    for y in window.y..window.bottom() {
        for x in window.x..window.right() {
            assert_eq!(
                byte(&got, x, y),
                byte(&a, x, y).saturating_sub(byte(&b, x, y)),
                "at ({x}, {y})"
            );
        }
    }
}

#[test]
fn a_border_wider_than_a_tiny_selection_is_just_its_expansion() {
    // The inner half erodes the 3 x 3 away, so nothing is subtracted.
    let ring = border(&solid(rect(0, 0, 3, 3)), 20).unwrap().unwrap();
    assert!(ring.bounds.contains_point(-5, 1));
    assert_eq!(byte(&ring, 1, 1), 255);
}

// ---- parameters, caps, bounds, determinism ----

#[test]
fn bad_parameters_are_refused() {
    let sel = solid(rect(0, 0, 5, 5));
    for bad in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        -1.0,
        MAX_FEATHER_PX + 0.5,
    ] {
        assert!(matches!(
            feather(&sel, bad),
            Err(DocError::SelectionRefineInvalid)
        ));
    }
    assert!(feather(&sel, MAX_FEATHER_PX).is_ok());
    assert!(matches!(
        expand(&sel, MAX_REFINE_PX + 1),
        Err(DocError::SelectionRefineInvalid)
    ));
    assert!(matches!(
        contract(&sel, MAX_REFINE_PX + 1),
        Err(DocError::SelectionRefineInvalid)
    ));
    assert!(matches!(
        border(&sel, MAX_REFINE_PX + 1),
        Err(DocError::SelectionRefineInvalid)
    ));
    assert!(matches!(
        border(&sel, 0),
        Err(DocError::SelectionRefineInvalid)
    ));
    assert!(matches!(
        smooth(&sel, MAX_SMOOTH_PX + 1),
        Err(DocError::SelectionRefineInvalid)
    ));
}

#[test]
fn a_result_over_the_mask_cap_is_refused_before_it_is_built() {
    // 2 x 16,777,215 expands by 1 to 4 x 16,777,217 = 64 Mpx + 4.
    let long = solid(rect(0, 0, 16_777_215, 2));
    assert!(matches!(
        expand(&long, 1),
        Err(DocError::SelectionMaskTooLarge(n)) if n == MAX_SELECTION_MASK_PIXELS + 4
    ));
    assert!(matches!(
        border(&long, 2),
        Err(DocError::SelectionMaskTooLarge(_))
    ));
    assert!(matches!(
        feather(&long, 10.0),
        Err(DocError::SelectionMaskTooLarge(_))
    ));
    // A rectangle over the cap is refused as an input too.
    assert!(matches!(
        smooth(&solid(rect(0, 0, 9000, 9000)), 1),
        Err(DocError::SelectionMaskTooLarge(_))
    ));
}

#[test]
fn the_cap_is_inclusive() {
    // A result of exactly 64 Mpx passes the cap check: 4 x 16,777,216.
    let grown = rect(-1, -1, 16_777_216, 4);
    assert_eq!(grown.area_px(), MAX_SELECTION_MASK_PIXELS);
    assert!(crate::selection_refine::cap_ok(&grown));
    assert!(!crate::selection_refine::cap_ok(&rect(
        -1, -1, 16_777_217, 4
    )));
}

#[test]
fn work_over_the_cap_is_refused_before_it_is_done() {
    // 5000 x 5000 = 25 Mpx at radius 100 is 25M * 204 steps.
    let big = solid(rect(0, 0, 5000, 5000));
    let Err(DocError::SelectionRefineTooExpensive(work)) = smooth(&big, 100) else {
        panic!("expected a work refusal");
    };
    assert!(work > MAX_REFINE_WORK);
    // Expand, Contract, Feather and Border have a texel budget too.
    let wide = solid(rect(0, 0, 6000, 6000));
    for result in [
        expand(&wide, 5),
        contract(&wide, 5),
        feather(&wide, 5.0),
        border(&wide, 5),
    ] {
        assert!(matches!(
            result,
            Err(DocError::SelectionRefineTooExpensive(_))
        ));
    }
    // Just inside the budget is not refused for work (a small one runs).
    assert!(expand(&solid(rect(0, 0, 300, 300)), 5).is_ok());
}

#[test]
fn results_have_tight_bounds_and_are_never_empty_and_plain() {
    let mut rng = Lcg(61);
    for _ in 0..40 {
        let sel = random_mask(&mut rng, 2, 2, 10, 8, true);
        let results = [
            expand(&sel, 1 + rng.below(3) as u32),
            contract(&sel, 1 + rng.below(3) as u32),
            smooth(&sel, 1 + rng.below(3) as u32),
            feather(&sel, 1.0 + rng.below(8) as f32),
            border(&sel, 1 + rng.below(5) as u32),
        ];
        for result in results {
            let Some(got) = result.unwrap() else { continue };
            assert!(!got.bounds.is_empty());
            assert!(!got.inverted);
            let b = got.bounds;
            let w = i64::from(b.width);
            let h = i64::from(b.height);
            let edge = |f: &dyn Fn(i64) -> (i64, i64), n: i64| {
                (0..n).any(|i| {
                    let (x, y) = f(i);
                    byte(&got, x, y) > 0
                })
            };
            assert!(edge(&|i| (b.x + i, b.y), w));
            assert!(edge(&|i| (b.x + i, b.bottom() - 1), w));
            assert!(edge(&|i| (b.x, b.y + i), h));
            assert!(edge(&|i| (b.right() - 1, b.y + i), h));
        }
    }
}

#[test]
fn a_result_that_is_solid_is_a_plain_rectangle() {
    let got = contract(&solid(rect(0, 0, 9, 9)), 2).unwrap().unwrap();
    assert!(matches!(got.shape, SelectionShape::Rect));
    assert_eq!(got.bounds, rect(2, 2, 5, 5));
}

#[test]
fn refinement_is_deterministic() {
    let mut rng = Lcg(71);
    let sel = random_mask(&mut rng, 0, 0, 24, 18, true);
    for _ in 0..2 {
        assert_eq!(expand(&sel, 3).unwrap(), expand(&sel, 3).unwrap());
        assert_eq!(contract(&sel, 3).unwrap(), contract(&sel, 3).unwrap());
        assert_eq!(feather(&sel, 5.5).unwrap(), feather(&sel, 5.5).unwrap());
        assert_eq!(smooth(&sel, 3).unwrap(), smooth(&sel, 3).unwrap());
        assert_eq!(border(&sel, 5).unwrap(), border(&sel, 5).unwrap());
    }
}

#[test]
fn ellipses_materialise_to_masks() {
    let e = Selection::ellipse(rect(0, 0, 21, 15));
    let got = expand(&e, 2).unwrap().unwrap();
    assert!(matches!(got.shape, SelectionShape::Mask(_)));
    assert_eq!(got.bounds, rect(-2, -2, 25, 19));
    assert_eq!(byte(&got, 10, 7), 255);
}

/// Timing of each command on a typical and a worst-case mask, printed for
/// PLAN.md. Ignored: run with `cargo test -p aurora-doc refine_cost --
/// --ignored --nocapture`.
#[test]
#[ignore = "a timing report, not a check"]
fn refine_cost_report() {
    use std::time::Instant;
    let mut rng = Lcg(99);
    for (w, h, label) in [(2000_u32, 2000_u32, "2000x2000"), (7900, 7900, "~62 Mpx")] {
        let data: Vec<u8> = (0..u64::from(w) * u64::from(h))
            .map(|i| {
                // Blobs rather than noise: a disk-ish field plus speckle.
                let (x, y) = ((i % u64::from(w)) as f64, (i / u64::from(w)) as f64);
                let v = ((x / 90.0).sin() + (y / 70.0).cos() + (rng.below(100) as f64) / 400.0)
                    * 80.0
                    + 128.0;
                v.clamp(0.0, 255.0) as u8
            })
            .collect();
        let sel = mask_selection(rect(0, 0, w, h), data);
        let timed = |name: &str, f: &dyn Fn() -> Result<Option<Selection>, DocError>| {
            let t = Instant::now();
            let r = f();
            println!(
                "{label} {name}: {:?} ({:?})",
                t.elapsed(),
                r.as_ref().map(|_| ()).map_err(|e| e.to_string())
            );
        };
        timed("expand 10", &|| expand(&sel, 10));
        timed("expand 100", &|| expand(&sel, 100));
        timed("contract 10", &|| contract(&sel, 10));
        timed("feather 10", &|| feather(&sel, 10.0));
        timed("feather 250", &|| feather(&sel, 250.0));
        timed("smooth 5", &|| smooth(&sel, 5));
        timed("smooth 20", &|| smooth(&sel, 20));
        timed("border 10", &|| border(&sel, 10));
    }
}

#[test]
fn expand_keeps_a_faint_texel_far_from_any_region() {
    // A texel below the 0.5 threshold with no region texel within reach is
    // not lowered to nothing by Expand.
    let mut data = vec![0_u8; 30 * 5];
    data[2 * 30 + 2] = 100;
    data[2 * 30 + 25] = 255;
    let got = expand(&mask_selection(rect(0, 0, 30, 5), data), 2)
        .unwrap()
        .unwrap();
    assert_eq!(byte(&got, 2, 2), 100);
    assert_eq!(byte(&got, 25, 2), 255);
}
