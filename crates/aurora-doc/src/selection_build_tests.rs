//! Tests of the polygon rasteriser and the antialiased binary mask
//! (0.214.0): exact areas, the nonzero winding rule, clipping, caps, tight
//! bounds, and a brute-force winding reference on random polygons.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_cmp
)]

use aurora_core::Rect;

use crate::{
    DocError, MAX_POLYGON_VERTICES, MAX_SELECTION_MASK_PIXELS, SelectionMask, antialiased_mask,
    polygon_mask,
};

fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

const BIG: Rect = Rect {
    x: -1000,
    y: -1000,
    width: 4000,
    height: 4000,
};

fn mask_of(points: &[(f64, f64)], clip: &Rect) -> SelectionMask {
    match polygon_mask(points, clip) {
        Ok(Some(mask)) => mask,
        other => unreachable!("expected a mask, got {other:?}"),
    }
}

fn sum_coverage(mask: &SelectionMask) -> f64 {
    let b = mask.bounds();
    let mut sum = 0.0;
    for y in b.y..b.bottom() {
        for x in b.x..b.right() {
            sum += f64::from(mask.byte_at(x, y)) / 255.0;
        }
    }
    sum
}

#[test]
fn a_pixel_aligned_rectangle_is_exactly_full_with_tight_bounds() {
    let mask = mask_of(&[(2.0, 3.0), (7.0, 3.0), (7.0, 9.0), (2.0, 9.0)], &BIG);
    assert_eq!(mask.bounds(), rect(2, 3, 5, 6));
    for y in 3..9 {
        for x in 2..7 {
            assert_eq!(mask.byte_at(x, y), 255, "({x}, {y})");
        }
    }
    assert_eq!(mask.byte_at(1, 3), 0);
    assert_eq!(mask.byte_at(7, 3), 0);
    assert_eq!(mask.byte_at(2, 9), 0);
}

#[test]
fn a_fractional_edge_gives_the_exact_fractional_coverage() {
    // The left edge at x = 2.5 covers half of column 2; the bottom edge at
    // y = 9.25 covers a quarter of row 9 (the vertical resolution).
    let mask = mask_of(&[(2.5, 3.0), (7.0, 3.0), (7.0, 9.25), (2.5, 9.25)], &BIG);
    assert_eq!(mask.bounds(), rect(2, 3, 5, 7));
    assert_eq!(mask.byte_at(2, 4), 128);
    assert_eq!(mask.byte_at(3, 4), 255);
    assert_eq!(mask.byte_at(4, 9), 64);
    assert_eq!(mask.byte_at(2, 9), 32);
}

#[test]
fn a_triangle_has_its_geometric_area() {
    let mask = mask_of(&[(0.0, 0.0), (8.0, 0.0), (0.0, 8.0)], &BIG);
    let area = sum_coverage(&mask);
    assert!((area - 32.0).abs() < 0.2, "area {area}");
    let mask = mask_of(&[(1.0, 1.0), (10.0, 2.0), (4.0, 12.0)], &BIG);
    let expected = 0.5 * ((10.0 - 1.0) * (12.0 - 1.0) - (4.0 - 1.0) * (2.0 - 1.0));
    let area = sum_coverage(&mask);
    assert!((area - expected).abs() < 0.3, "{area} vs {expected}");
}

#[test]
fn vertex_order_and_start_do_not_matter() {
    let tri = [(1.0, 1.0), (10.0, 2.0), (4.0, 12.0)];
    let a = mask_of(&tri, &BIG);
    let reversed = [tri[2], tri[1], tri[0]];
    let rotated = [tri[1], tri[2], tri[0]];
    assert_eq!(a, mask_of(&reversed, &BIG));
    assert_eq!(a, mask_of(&rotated, &BIG));
}

#[test]
fn a_figure_eight_fills_both_lobes() {
    // Nonzero winding: the two lobes wind in opposite directions, and both
    // are inside.
    let mask = mask_of(&[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)], &BIG);
    assert_eq!(mask.byte_at(1, 5), 255, "left lobe");
    assert_eq!(mask.byte_at(8, 5), 255, "right lobe");
    assert_eq!(mask.byte_at(5, 1), 0, "above the crossing");
    assert_eq!(mask.byte_at(5, 8), 0, "below the crossing");
}

/// An outer square, a bridge in to (3, 3), an inner square, and the
/// bridge back (the two bridge edges cancel).
fn square_with_inner(inner_same_direction: bool) -> Vec<(f64, f64)> {
    let mut points = vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (0.0, 10.0),
        (0.0, 0.0),
    ];
    if inner_same_direction {
        points.extend([(3.0, 3.0), (7.0, 3.0), (7.0, 7.0), (3.0, 7.0), (3.0, 3.0)]);
    } else {
        points.extend([(3.0, 3.0), (3.0, 7.0), (7.0, 7.0), (7.0, 3.0), (3.0, 3.0)]);
    }
    points
}

#[test]
fn nonzero_winding_fills_a_same_direction_inner_loop_and_holes_an_opposite_one() {
    let filled = mask_of(&square_with_inner(true), &BIG);
    assert_eq!(filled.byte_at(5, 5), 255, "winding 2 is inside");
    assert_eq!(filled.byte_at(1, 1), 255);
    let holed = mask_of(&square_with_inner(false), &BIG);
    assert_eq!(holed.byte_at(5, 5), 0, "winding 0 is a hole");
    assert_eq!(holed.byte_at(1, 1), 255);
    assert_eq!(holed.byte_at(8, 8), 255);
    assert_eq!(holed.byte_at(3, 3), 0);
}

#[test]
fn degenerate_polygons_select_nothing() {
    for points in [
        vec![],
        vec![(1.0, 1.0)],
        vec![(1.0, 1.0), (5.0, 5.0)],
        vec![(1.0, 1.0), (5.0, 5.0), (9.0, 9.0)],
        vec![(2.0, 2.0), (2.0, 2.0), (2.0, 2.0)],
        vec![(0.0, 3.0), (5.0, 3.0), (9.0, 3.0)],
        vec![(f64::NAN, 0.0), (f64::INFINITY, 4.0), (3.0, f64::NAN)],
    ] {
        assert!(
            matches!(polygon_mask(&points, &BIG), Ok(None)),
            "{points:?}"
        );
    }
}

#[test]
fn a_polygon_is_clipped_to_the_clip_rectangle() {
    let clip = rect(0, 0, 10, 10);
    let mask = mask_of(&[(-5.0, -5.0), (5.0, -5.0), (5.0, 5.0), (-5.0, 5.0)], &clip);
    assert_eq!(mask.bounds(), rect(0, 0, 5, 5));
    assert!(matches!(
        polygon_mask(&[(20.0, 20.0), (30.0, 20.0), (30.0, 30.0)], &clip),
        Ok(None)
    ));
}

#[test]
fn huge_coordinates_are_clamped_and_nonfinite_vertices_dropped() {
    let clip = rect(0, 0, 8, 8);
    let huge = 1.0e300;
    let mask = mask_of(
        &[(-huge, -huge), (huge, -huge), (huge, huge), (-huge, huge)],
        &clip,
    );
    assert_eq!(mask.bounds(), clip);
    assert_eq!(mask.byte_at(7, 7), 255);
    // A NaN vertex among good ones is skipped, the rest still selected.
    let mask = mask_of(
        &[
            (1.0, 1.0),
            (f64::NAN, 3.0),
            (5.0, 1.0),
            (5.0, 5.0),
            (1.0, 5.0),
        ],
        &clip,
    );
    assert_eq!(mask.bounds(), rect(1, 1, 4, 4));
}

#[test]
fn the_vertex_cap_and_the_mask_cap_refuse() {
    let many = vec![(1.0, 1.0); MAX_POLYGON_VERTICES + 1];
    assert!(matches!(
        polygon_mask(&many, &BIG),
        Err(DocError::SelectionPolygonTooComplex(n)) if n == MAX_POLYGON_VERTICES + 1
    ));
    let at_cap = vec![(1.0, 1.0); MAX_POLYGON_VERTICES];
    assert!(matches!(polygon_mask(&at_cap, &BIG), Ok(None)));
    // 10,000 x 10,000 = 100 Mpx covered: refused before allocating.
    let clip = rect(0, 0, 10_000, 10_000);
    let result = polygon_mask(
        &[
            (0.0, 0.0),
            (10_000.0, 0.0),
            (10_000.0, 10_000.0),
            (0.0, 10_000.0),
        ],
        &clip,
    );
    assert!(
        matches!(result, Err(DocError::SelectionMaskTooLarge(n)) if n > MAX_SELECTION_MASK_PIXELS),
        "{result:?}"
    );
}

#[test]
fn bounds_are_the_tightest_rectangle_of_nonzero_texels() {
    let mask = mask_of(&[(3.2, 4.2), (9.7, 5.1), (6.0, 11.4)], &BIG);
    let b = mask.bounds();
    let touches = |f: &dyn Fn(i64, i64) -> bool| -> bool {
        (b.y..b.bottom()).any(|y| (b.x..b.right()).any(|x| f(x, y) && mask.byte_at(x, y) > 0))
    };
    assert!(touches(&|x, _| x == b.x), "left column used");
    assert!(touches(&|x, _| x == b.right() - 1), "right column used");
    assert!(touches(&|_, y| y == b.y), "top row used");
    assert!(touches(&|_, y| y == b.bottom() - 1), "bottom row used");
}

/// The winding number of `points` around `(px, py)`.
fn winding_at(points: &[(f64, f64)], px: f64, py: f64) -> i32 {
    let mut winding = 0;
    for (i, &(ax, ay)) in points.iter().enumerate() {
        let (bx, by) = points[(i + 1) % points.len()];
        let crosses_down = ay <= py && by > py;
        let crosses_up = by <= py && ay > py;
        if !(crosses_down || crosses_up) {
            continue;
        }
        let x = ax + (py - ay) * (bx - ax) / (by - ay);
        if x > px {
            winding += if crosses_down { 1 } else { -1 };
        }
    }
    winding
}

#[test]
fn random_polygons_agree_with_a_brute_force_winding_reference() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        f64::from(u32::try_from(state >> 40).unwrap_or(0)) / f64::from(1_u32 << 24)
    };
    let clip = rect(0, 0, 24, 24);
    let mut compared = 0;
    for _ in 0..40 {
        let count = 3 + (next() * 7.0) as usize;
        let points: Vec<(f64, f64)> = (0..count).map(|_| (next() * 24.0, next() * 24.0)).collect();
        let mask = match polygon_mask(&points, &clip) {
            Ok(Some(mask)) => mask,
            Ok(None) => continue,
            Err(e) => unreachable!("{e}"),
        };
        let (mut total_error, mut worst) = (0.0_f64, 0.0_f64);
        for y in 0..24_i64 {
            for x in 0..24_i64 {
                // 16 x 16 sample points per pixel.
                let mut inside = 0;
                for sy in 0..16 {
                    for sx in 0..16 {
                        let px = x as f64 + (f64::from(sx) + 0.5) / 16.0;
                        let py = y as f64 + (f64::from(sy) + 0.5) / 16.0;
                        if winding_at(&points, px, py) != 0 {
                            inside += 1;
                        }
                    }
                }
                let reference = f64::from(inside) / 256.0;
                let error = (f64::from(mask.byte_at(x, y)) / 255.0 - reference).abs();
                total_error += error;
                worst = worst.max(error);
                // Deep inside or outside is exact.
                if reference == 0.0 || reference == 1.0 {
                    assert!(error < 0.3, "pixel ({x}, {y}) ref {reference} {points:?}");
                }
            }
        }
        assert!(worst <= 0.55, "worst {worst} for {points:?}");
        assert!(
            total_error / 576.0 < 0.03,
            "mean {} {points:?}",
            total_error / 576.0
        );
        compared += 1;
    }
    assert!(compared >= 30, "{compared} polygons compared");
}

#[test]
fn the_same_polygon_gives_the_same_mask_twice() {
    let points = [(1.3, 1.1), (14.9, 3.3), (9.2, 17.7), (2.2, 12.4)];
    assert_eq!(mask_of(&points, &BIG), mask_of(&points, &BIG));
}

#[test]
fn the_antialiased_mask_softens_only_the_outside_of_a_block() {
    let bounds = rect(10, 20, 6, 5);
    let data = vec![255_u8; 30];
    let mask = match antialiased_mask(bounds, &data) {
        Ok(Some(mask)) => mask,
        other => unreachable!("{other:?}"),
    };
    assert_eq!(mask.bounds(), rect(9, 19, 8, 7), "grown by one texel");
    assert_eq!(mask.byte_at(12, 22), 255, "interior is full");
    assert_eq!(mask.byte_at(10, 22), 255, "selected texels stay full");
    assert_eq!(mask.byte_at(9, 22), 64, "outside edge: one quarter");
    assert_eq!(mask.byte_at(12, 19), 64, "above the block");
    assert_eq!(mask.byte_at(12, 25), 64, "below the block");
    // Corners: the selected corner texel stays full, the outside diagonal
    // gets a sixteenth.
    assert_eq!(mask.byte_at(10, 20), 255);
    assert_eq!(mask.byte_at(9, 19), 16);
    // The half threshold keeps the original shape exactly.
    for y in 19..28 {
        for x in 9..18 {
            let was = (10..16).contains(&x) && (20..25).contains(&y);
            assert_eq!(mask.byte_at(x, y) >= 128, was, "({x}, {y})");
        }
    }
}

#[test]
fn a_single_texel_becomes_a_three_by_three_blur_and_nothing_is_none() {
    let mask = match antialiased_mask(rect(5, 5, 1, 1), &[255]) {
        Ok(Some(mask)) => mask,
        other => unreachable!("{other:?}"),
    };
    assert_eq!(mask.bounds(), rect(4, 4, 3, 3));
    assert_eq!(
        mask.byte_at(5, 5),
        255,
        "a one-pixel region stays fully selected"
    );
    assert_eq!(mask.byte_at(4, 5), 32);
    assert_eq!(mask.byte_at(5, 4), 32, "above the texel");
    assert_eq!(mask.byte_at(5, 6), 32, "below the texel");
    assert_eq!(mask.byte_at(4, 4), 16);
    assert!(matches!(
        antialiased_mask(rect(0, 0, 2, 2), &[0; 4]),
        Ok(None)
    ));
    assert!(matches!(
        antialiased_mask(rect(0, 0, 2, 2), &[255; 3]),
        Err(DocError::SelectionMaskInvalid)
    ));
    assert!(matches!(
        antialiased_mask(rect(0, 0, 0, 2), &[]),
        Err(DocError::SelectionMaskInvalid)
    ));
}
