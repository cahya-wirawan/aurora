#![allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::needless_range_loop,
    clippy::manual_midpoint,
    clippy::identity_op
)]

use std::sync::Arc;

use aurora_core::Rect;

use super::*;
use crate::selection::{Selection, SelectionShape};
use crate::selection_mask::SelectionMask;
use crate::{polygon_mask, selection_from_path};

const SIZE: (u32, u32) = (400, 400);

fn rect(x: i64, y: i64, w: u32, h: u32) -> Rect {
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

fn mask_selection(bounds: Rect, f: impl Fn(i64, i64) -> f32) -> Selection {
    let mask = SelectionMask::rasterize(bounds, f).unwrap_or_else(|_| unreachable!("mask"));
    Selection::from_mask(Arc::new(mask))
}

/// Intersection over union of the `>= 0.5` regions of two selections.
fn iou(a: &Selection, b: &Selection, size: (u32, u32)) -> f64 {
    let (mut inter, mut union) = (0_u64, 0_u64);
    for y in 0..i64::from(size.1) {
        for x in 0..i64::from(size.0) {
            let (ia, ib) = (a.coverage_at(x, y) >= 0.5, b.coverage_at(x, y) >= 0.5);
            inter += u64::from(ia && ib);
            union += u64::from(ia || ib);
        }
    }
    if union == 0 {
        1.0
    } else {
        inter as f64 / union as f64
    }
}

fn round_trip(sel: &Selection) -> Selection {
    let path = path_from_selection(sel, SIZE).unwrap_or_else(|e| unreachable!("path: {e:?}"));
    selection_from_path(&path, SIZE)
        .unwrap_or_else(|e| unreachable!("fill: {e:?}"))
        .unwrap_or_else(|| unreachable!("non-empty"))
}

#[test]
fn a_rectangle_gives_its_four_corners_and_round_trips_exactly() {
    let sel = Selection {
        bounds: rect(30, 40, 120, 80),
        inverted: false,
        shape: SelectionShape::Rect,
    };
    let path = path_from_selection(&sel, SIZE).unwrap_or_else(|_| unreachable!());
    assert_eq!(path.name, WORK_PATH_NAME);
    assert_eq!(path.subpaths.len(), 1);
    let pts: Vec<_> = path.subpaths[0].anchors.iter().map(|a| a.point).collect();
    assert_eq!(
        pts,
        vec![(30.0, 40.0), (150.0, 40.0), (150.0, 120.0), (30.0, 120.0)]
    );
    assert!(path.subpaths[0].anchors.iter().all(Anchor::is_corner));
    let back = round_trip(&sel);
    for y in 0..i64::from(SIZE.1) {
        for x in 0..i64::from(SIZE.0) {
            assert_eq!(
                back.coverage_at(x, y) >= 0.5,
                sel.coverage_at(x, y) >= 0.5,
                "({x},{y})"
            );
        }
    }
}

#[test]
fn an_ellipse_is_a_four_anchor_bezier_circle() {
    let sel = Selection {
        bounds: rect(50, 60, 200, 120),
        inverted: false,
        shape: SelectionShape::Ellipse,
    };
    let path = path_from_selection(&sel, SIZE).unwrap_or_else(|_| unreachable!());
    assert_eq!(path.subpaths[0].anchors.len(), 4);
    assert!(path.subpaths[0].closed);
    let right = path.subpaths[0].anchors[0];
    assert_eq!(right.point, (250.0, 120.0));
    assert!((right.handle_out.1 - 60.0 * 0.5523).abs() < 0.01);
    assert!(iou(&sel, &round_trip(&sel), SIZE) >= 0.99);
}

fn star(cx: f64, cy: f64) -> Vec<(f64, f64)> {
    (0..28)
        .map(|i| {
            let angle = f64::from(i) * std::f64::consts::TAU / 28.0;
            let r = if i % 2 == 0 {
                150.0
            } else {
                80.0 + f64::from(i % 5) * 9.0
            };
            (cx + r * angle.cos(), cy + r * angle.sin())
        })
        .collect()
}

#[test]
fn a_lasso_like_mask_round_trips_with_high_overlap() {
    let clip = rect(0, 0, 400, 400);
    let mask = polygon_mask(&star(200.0, 200.0), &clip)
        .unwrap_or_else(|_| unreachable!())
        .unwrap_or_else(|| unreachable!());
    let sel = Selection::from_mask(Arc::new(mask));
    let path = path_from_selection(&sel, SIZE).unwrap_or_else(|_| unreachable!());
    assert_eq!(path.subpaths.len(), 1);
    assert!(
        path.subpaths[0].anchors.len() < 300,
        "simplified: {}",
        path.subpaths[0].anchors.len()
    );
    let value = iou(&sel, &round_trip(&sel), SIZE);
    assert!(value >= 0.98, "IoU {value}");
}

#[test]
fn holes_are_separate_opposite_subpaths_and_survive() {
    let sel = mask_selection(rect(20, 20, 300, 300), |x, y| {
        let d = ((x - 170) as f64).hypot((y - 170) as f64);
        if (40.0..120.0).contains(&d) { 1.0 } else { 0.0 }
    });
    let path = path_from_selection(&sel, SIZE).unwrap_or_else(|_| unreachable!());
    assert_eq!(path.subpaths.len(), 2, "outer ring and hole");
    let area = |s: &Subpath| {
        let p: Vec<_> = s
            .anchors
            .iter()
            .map(|a| (f64::from(a.point.0), f64::from(a.point.1)))
            .collect();
        signed_area2(&p)
    };
    let (a, b) = (area(&path.subpaths[0]), area(&path.subpaths[1]));
    assert!(a * b < 0.0, "opposite windings: {a} {b}");
    let back = round_trip(&sel);
    assert!(back.coverage_at(170, 170) < 0.5, "the hole stays a hole");
    assert!(back.coverage_at(170, 100) >= 0.5);
    assert!(iou(&sel, &back, SIZE) >= 0.98);
}

#[test]
fn diagonal_pixels_trace_as_two_loops() {
    let sel = mask_selection(
        rect(10, 10, 2, 2),
        |x, y| if (x + y) % 2 == 0 { 1.0 } else { 0.0 },
    );
    let mask = match &sel.shape {
        SelectionShape::Mask(m) => Arc::clone(m),
        _ => unreachable!("mask"),
    };
    let loops = trace_loops(&mask).unwrap_or_else(|| unreachable!());
    assert_eq!(loops.len(), 2);
    assert!(loops.iter().all(|l| l.len() == 4));
}

#[test]
fn a_straight_run_through_the_start_vertex_is_not_a_corner() {
    let sel = mask_selection(rect(0, 0, 5, 3), |_, _| 1.0);
    let mask = match &sel.shape {
        SelectionShape::Mask(m) => Arc::clone(m),
        _ => unreachable!("mask"),
    };
    let loops = trace_loops(&mask).unwrap_or_else(|| unreachable!());
    assert_eq!(loops, vec![vec![(0, 0), (5, 0), (5, 3), (0, 3)]]);
}

#[test]
fn an_inverted_selection_is_the_document_plus_reversed_holes() {
    let mut sel = Selection {
        bounds: rect(100, 100, 100, 100),
        inverted: false,
        shape: SelectionShape::Rect,
    };
    sel.inverted = true;
    let path = path_from_selection(&sel, SIZE).unwrap_or_else(|_| unreachable!());
    assert_eq!(path.subpaths.len(), 2);
    assert_eq!(path.subpaths[0].anchors[2].point, (400.0, 400.0));
    let back = round_trip(&sel);
    assert!(back.coverage_at(150, 150) < 0.5);
    assert!(back.coverage_at(10, 10) >= 0.5);
    assert!(back.coverage_at(399, 399) >= 0.5);
    assert!(iou(&sel, &back, SIZE) >= 0.999);
}

#[test]
fn an_empty_mask_is_refused_as_empty() {
    let sel = mask_selection(rect(0, 0, 10, 10), |_, _| 0.0);
    assert_eq!(
        path_from_selection(&sel, SIZE),
        Err(PathFromSelectionError::Empty)
    );
    let zero = Selection {
        bounds: rect(5, 5, 0, 9),
        inverted: false,
        shape: SelectionShape::Rect,
    };
    assert_eq!(
        path_from_selection(&zero, SIZE),
        Err(PathFromSelectionError::Empty)
    );
}

#[test]
fn too_many_loops_are_simplified_away_then_refused() {
    // A checkerboard of one-pixel squares: every loop is a speck.
    let sel = mask_selection(rect(0, 0, 200, 200), |x, y| {
        if (x / 1 + y) % 2 == 0 && x % 2 == 0 {
            1.0
        } else {
            0.0
        }
    });
    assert_eq!(
        path_from_selection(&sel, SIZE),
        Err(PathFromSelectionError::TooComplex)
    );
}

#[test]
fn simplification_tolerance_bounds_the_error() {
    let ring: Vec<(f64, f64)> = (0..360)
        .map(|i| {
            let a = f64::from(i).to_radians();
            (100.0 + 90.0 * a.cos(), 100.0 + 90.0 * a.sin())
        })
        .collect();
    let simple = simplify_closed(&ring, 0.5);
    assert!(simple.len() < 60 && simple.len() >= 8, "{}", simple.len());
    // Every dropped point lies within the tolerance of the kept polygon.
    for p in &ring {
        let d = (0..simple.len())
            .map(|i| {
                let (a, b) = (simple[i], simple[(i + 1) % simple.len()]);
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let t =
                    (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                (p.0 - (a.0 + t * dx)).hypot(p.1 - (a.1 + t * dy))
            })
            .fold(f64::MAX, f64::min);
        assert!(d <= 0.5 + 1e-9, "{d}");
    }
}

#[test]
fn a_single_pixel_selection_is_kept() {
    let sel = mask_selection(rect(7, 9, 1, 1), |_, _| 1.0);
    let path = path_from_selection(&sel, SIZE).unwrap_or_else(|_| unreachable!());
    assert_eq!(path.subpaths.len(), 1);
    assert_eq!(path.subpaths[0].anchors.len(), 4);
}
