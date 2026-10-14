//! Tests of the pure parts of Fill Path, Stroke Path and Make Work Path
//! from Selection (0.234.0): the dab stepping, the cap, the availability
//! rules and the work-path step. The pixel flows are in `fill_tests.rs`.

#![allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::cast_precision_loss
)]

use std::sync::Arc;

use aurora_core::Rect;
use aurora_doc::{Anchor, Path, PathSet, Selection, SelectionShape, Subpath, WORK_PATH_NAME};

use super::*;

fn line(points: &[(f32, f32)], closed: bool) -> FlatSubpath {
    (points.to_vec(), closed)
}

fn near(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
}

#[test]
fn a_straight_open_subpath_has_dabs_a_step_apart_including_both_ends() {
    let out = path_dabs(&[line(&[(0.0, 0.0), (100.0, 0.0)], false)], 10.0, 1000);
    assert!(!out.truncated);
    assert_eq!(out.dabs.len(), 11);
    for (i, dab) in out.dabs.iter().enumerate() {
        assert!(near(*dab, (i as f32 * 10.0, 0.0)), "{i}: {dab:?}");
    }
}

#[test]
fn the_spacing_carries_across_segments() {
    let out = path_dabs(
        &[line(&[(0.0, 0.0), (25.0, 0.0), (25.0, 25.0)], false)],
        10.0,
        1000,
    );
    let expected = [
        (0.0, 0.0),
        (10.0, 0.0),
        (20.0, 0.0),
        (25.0, 5.0),
        (25.0, 15.0),
        (25.0, 25.0),
    ];
    assert_eq!(out.dabs.len(), expected.len(), "{:?}", out.dabs);
    for (dab, want) in out.dabs.iter().zip(expected) {
        assert!(near(*dab, want), "{dab:?} vs {want:?}");
    }
}

#[test]
fn a_closed_subpath_walks_its_closing_segment_and_does_not_double_the_start() {
    let square = [(0.0, 0.0), (40.0, 0.0), (40.0, 40.0), (0.0, 40.0)];
    let closed = path_dabs(&[line(&square, true)], 10.0, 1000);
    assert_eq!(closed.dabs.len(), 16, "perimeter 160 / 10, the start once");
    assert!(
        closed.dabs.iter().any(|d| near(*d, (0.0, 30.0))),
        "on the closing side"
    );
    let open = path_dabs(&[line(&square, false)], 10.0, 1000);
    assert_eq!(open.dabs.len(), 13, "three sides only");
    assert!(!open.dabs.iter().any(|d| near(*d, (0.0, 30.0))));
}

#[test]
fn every_subpath_restarts_with_a_dab_on_its_first_point() {
    let out = path_dabs(
        &[
            line(&[(0.0, 0.0), (25.0, 0.0)], false),
            line(&[(0.0, 100.0), (25.0, 100.0)], false),
        ],
        10.0,
        1000,
    );
    let ys: Vec<f32> = out.dabs.iter().map(|d| d.1).collect();
    assert_eq!(out.dabs.len(), 6);
    assert_eq!(ys, vec![0.0, 0.0, 0.0, 100.0, 100.0, 100.0]);
    assert!(
        near(out.dabs[3], (0.0, 100.0)),
        "no carry from the first subpath"
    );
}

#[test]
fn degenerate_subpaths_and_steps() {
    let one = path_dabs(&[line(&[(7.0, 8.0)], false)], 10.0, 100);
    assert_eq!(one.dabs, vec![(7.0, 8.0)]);
    let same = path_dabs(
        &[line(&[(7.0, 8.0), (7.0, 8.0), (7.0, 8.0)], true)],
        10.0,
        100,
    );
    assert_eq!(same.dabs, vec![(7.0, 8.0)], "zero length is one dab");
    let empty = path_dabs(&[line(&[], false)], 10.0, 100);
    assert!(empty.dabs.is_empty() && !empty.truncated);
    for step in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(
            path_dabs(&[line(&[(0.0, 0.0), (9.0, 9.0)], false)], step, 100)
                .dabs
                .is_empty()
        );
    }
}

#[test]
fn the_cap_truncates_and_says_so() {
    let long = [line(&[(0.0, 0.0), (100.0, 0.0)], false)];
    let cut = path_dabs(&long, 1.0, 10);
    assert_eq!(cut.dabs.len(), 10);
    assert!(cut.truncated);
    let exact = path_dabs(&long, 10.0, 11);
    assert_eq!(exact.dabs.len(), 11);
    assert!(!exact.truncated, "exactly the cap is fine");
    let many = path_dabs(
        &[line(&[(0.0, 0.0)], false), line(&[(1.0, 1.0)], false)],
        1.0,
        1,
    );
    assert!(
        many.truncated && many.dabs.len() == 1,
        "a second subpath past the cap"
    );
}

#[test]
fn the_dab_cap_shrinks_with_the_brush_and_stays_within_bounds() {
    assert_eq!(stroke_dab_cap(1.0), MAX_STROKE_DABS);
    assert_eq!(stroke_dab_cap(5000.0), MIN_STROKE_DABS);
    let (small, big) = (stroke_dab_cap(24.0), stroke_dab_cap(100.0));
    assert!(
        big < small && small < MAX_STROKE_DABS && big > MIN_STROKE_DABS,
        "{small} {big}"
    );
    assert_eq!(stroke_dab_cap(f32::NAN), MAX_STROKE_DABS);
}

fn poly(n: usize) -> Path {
    Path {
        name: "p".into(),
        subpaths: vec![Subpath {
            anchors: (0..n)
                .map(|i| Anchor::corner(i as f32 * 10.0, 0.0))
                .collect(),
            closed: false,
        }],
    }
}

#[test]
fn fill_needs_three_points_and_stroke_two() {
    assert!(!path_can_fill(&poly(2)) && path_can_fill(&poly(3)));
    assert!(!path_can_stroke(&poly(1)) && path_can_stroke(&poly(2)));
    assert!(!path_can_stroke(&Path::new("empty")));
    let two_subpaths = Path {
        name: "q".into(),
        subpaths: vec![poly(2).subpaths.remove(0), poly(1).subpaths.remove(0)],
    };
    assert!(path_can_stroke(&two_subpaths) && !path_can_fill(&two_subpaths));
}

fn rect_selection(x: i64, y: i64, w: u32, h: u32) -> Selection {
    Selection {
        bounds: Rect {
            x,
            y,
            width: w,
            height: h,
        },
        inverted: false,
        shape: SelectionShape::Rect,
    }
}

#[test]
fn make_work_path_creates_replaces_and_skips_an_identical_one() {
    let mut paths = PathSet::new();
    let sel = rect_selection(10, 20, 100, 50);
    let step = make_work_path(&mut paths, &sel, (400, 400)).unwrap_or_else(|e| unreachable!("{e}"));
    let Some(step) = step else {
        unreachable!("a step");
    };
    assert_eq!(step.label, MAKE_WORK_PATH_LABEL);
    assert!(step.before.is_empty());
    let work = paths.work().unwrap_or_else(|| unreachable!("work path"));
    assert_eq!(work.name, WORK_PATH_NAME);
    assert_eq!(work.subpaths[0].anchors.len(), 4);
    // The same selection again changes nothing.
    assert_eq!(make_work_path(&mut paths, &sel, (400, 400)), Ok(None));
    // A different one replaces the work path (and leaves saved paths alone).
    assert!(paths.add("Saved").is_ok());
    let other = rect_selection(0, 0, 30, 30);
    let replaced = make_work_path(&mut paths, &other, (400, 400));
    let Ok(Some(step)) = replaced else {
        unreachable!("a step");
    };
    assert_eq!(step.before.work().map(|p| p.subpaths.len()), Some(1));
    assert_eq!(
        paths.work().map(|p| p.subpaths[0].anchors[2].point),
        Some((30.0, 30.0))
    );
    assert!(paths.get("Saved").is_some());
}

#[test]
fn make_work_path_names_an_empty_selection() {
    let mut paths = PathSet::new();
    let mask = aurora_doc::SelectionMask::rasterize(
        Rect {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        },
        |_, _| 0.0,
    );
    let Ok(mask) = mask else {
        unreachable!("mask");
    };
    let sel = Selection::from_mask(Arc::new(mask));
    assert_eq!(
        make_work_path(&mut paths, &sel, (100, 100)),
        Err(NOTHING_SELECTED_NOTICE)
    );
    assert!(paths.is_empty(), "nothing was created");
}
