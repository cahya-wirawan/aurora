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

use super::*;
use crate::path::Anchor;

fn smooth(p: (f32, f32), out: (f32, f32)) -> Anchor {
    Anchor::smooth(p, out)
}

fn point_at(a: &Anchor, b: &Anchor, t: f32) -> (f32, f32) {
    super::curve_point(a, b, t)
}

fn near(a: (f32, f32), b: (f32, f32), eps: f32) -> bool {
    (a.0 - b.0).abs() <= eps && (a.1 - b.1).abs() <= eps
}

fn open(points: &[(f32, f32)]) -> Subpath {
    Subpath {
        anchors: points.iter().map(|&(x, y)| Anchor::corner(x, y)).collect(),
        closed: false,
    }
}

#[test]
fn split_traces_exactly_the_original_curve() {
    let a = smooth((0.0, 0.0), (30.0, 40.0));
    let b = smooth((100.0, 20.0), (30.0, -40.0));
    for t in [0.1_f32, 0.25, 0.5, 0.8] {
        let (first, mid, last) = split_segment(&a, &b, t);
        for u in 0..=10 {
            let u = u as f32 / 10.0;
            assert!(
                near(point_at(&first, &mid, u), point_at(&a, &b, u * t), 1e-3),
                "first half at t={t} u={u}"
            );
            assert!(
                near(
                    point_at(&mid, &last, u),
                    point_at(&a, &b, t + u * (1.0 - t)),
                    1e-3
                ),
                "second half at t={t} u={u}"
            );
        }
        assert!(near(mid.point, point_at(&a, &b, t), 1e-3));
        // The outer anchors keep their points and their other handles.
        assert_eq!(first.point, a.point);
        assert_eq!(first.handle_in, a.handle_in);
        assert_eq!(last.handle_out, b.handle_out);
    }
}

#[test]
fn splitting_a_straight_segment_gives_a_corner_on_the_line() {
    let a = Anchor::corner(0.0, 0.0);
    let b = Anchor::corner(100.0, 40.0);
    let (first, mid, last) = split_segment(&a, &b, 0.25);
    assert_eq!(mid.point, (25.0, 10.0));
    assert!(mid.is_corner());
    assert_eq!((first, last), (a, b));
}

#[test]
fn nearest_segment_finds_the_line_within_the_radius_only() {
    let subs = [open(&[(0.0, 0.0), (100.0, 0.0)])];
    let hit = nearest_segment(&subs, (25.0, 3.0), 5.0);
    let hit = hit.unwrap_or_else(|| unreachable!("a hit"));
    assert_eq!((hit.subpath, hit.segment), (0, 0));
    assert!((hit.t - 0.25).abs() < 1e-3, "t = {}", hit.t);
    assert!((hit.distance - 3.0).abs() < 1e-3);
    assert!(nearest_segment(&subs, (25.0, 6.0), 5.0).is_none());
    assert!(
        nearest_segment(&subs, (150.0, 0.0), 5.0).is_none(),
        "past the end"
    );
}

#[test]
fn nearest_segment_includes_the_closing_segment_only_when_closed() {
    let mut sub = open(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)]);
    let on_closing = (50.0, 50.0);
    assert!(nearest_segment(std::slice::from_ref(&sub), on_closing, 3.0).is_none());
    sub.closed = true;
    let hit = nearest_segment(&[sub], on_closing, 3.0);
    assert_eq!(hit.map(|h| h.segment), Some(2));
}

#[test]
fn nearest_segment_picks_the_closer_of_two() {
    let subs = [
        open(&[(0.0, 0.0), (100.0, 0.0)]),
        open(&[(0.0, 4.0), (100.0, 4.0)]),
    ];
    let hit = nearest_segment(&subs, (50.0, 3.0), 10.0);
    assert_eq!(hit.map(|h| h.subpath), Some(1));
}

#[test]
fn insert_anchor_splits_in_place_and_keeps_the_shape() {
    let mut subs = vec![Subpath {
        anchors: vec![
            smooth((0.0, 0.0), (30.0, 40.0)),
            smooth((100.0, 20.0), (30.0, -40.0)),
            Anchor::corner(200.0, 0.0),
        ],
        closed: false,
    }];
    let before = subs.clone();
    let hit = SegmentHit {
        subpath: 0,
        segment: 0,
        t: 0.4,
        distance: 0.0,
    };
    assert_eq!(insert_anchor(&mut subs, &hit), Some(1));
    let anchors = &subs[0].anchors;
    assert_eq!(anchors.len(), 4);
    assert_eq!(anchors[2].point, before[0].anchors[1].point);
    let orig = point_at(&before[0].anchors[0], &before[0].anchors[1], 0.4);
    assert!(near(anchors[1].point, orig, 1e-3));
    // The untouched segment is untouched.
    assert_eq!(anchors[3], before[0].anchors[2]);
}

#[test]
fn insert_anchor_on_the_closing_segment_appends() {
    let mut subs = vec![Subpath {
        anchors: vec![
            Anchor::corner(0.0, 0.0),
            Anchor::corner(100.0, 0.0),
            Anchor::corner(100.0, 100.0),
        ],
        closed: true,
    }];
    let hit = SegmentHit {
        subpath: 0,
        segment: 2,
        t: 0.5,
        distance: 0.0,
    };
    assert_eq!(insert_anchor(&mut subs, &hit), Some(3));
    assert_eq!(subs[0].anchors.len(), 4);
    assert_eq!(subs[0].anchors[3].point, (50.0, 50.0));
    assert!(subs[0].closed);
    // An open subpath has no closing segment.
    let mut open_subs = vec![open(&[(0.0, 0.0), (10.0, 0.0)])];
    let bad = SegmentHit {
        subpath: 0,
        segment: 1,
        t: 0.5,
        distance: 0.0,
    };
    assert_eq!(insert_anchor(&mut open_subs, &bad), None);
}

#[test]
fn corner_becomes_smooth_with_third_length_handles_and_back() {
    let mut sub = open(&[(0.0, 0.0), (30.0, 0.0), (30.0, 60.0)]);
    assert!(toggle_corner_smooth(&mut sub, 1));
    let a = sub.anchors[1];
    // Direction from previous (0,0) to next (30,60), unit.
    let len = 30.0_f32.hypot(60.0);
    let unit = (30.0 / len, 60.0 / len);
    assert!(
        near(a.handle_out, (unit.0 * 20.0, unit.1 * 20.0), 1e-3),
        "out {:?}",
        a.handle_out
    );
    assert!(
        near(a.handle_in, (-unit.0 * 10.0, -unit.1 * 10.0), 1e-3),
        "in {:?}",
        a.handle_in
    );
    assert!(toggle_corner_smooth(&mut sub, 1));
    assert!(sub.anchors[1].is_corner());
}

#[test]
fn toggle_handles_ends_closed_wrap_and_lone_anchors() {
    let mut sub = open(&[(0.0, 0.0), (30.0, 0.0)]);
    assert!(toggle_corner_smooth(&mut sub, 0));
    assert_eq!(
        sub.anchors[0].handle_in,
        (0.0, 0.0),
        "no previous neighbour"
    );
    assert!(near(sub.anchors[0].handle_out, (10.0, 0.0), 1e-4));
    let mut closed = Subpath {
        anchors: vec![
            Anchor::corner(0.0, 0.0),
            Anchor::corner(30.0, 0.0),
            Anchor::corner(30.0, 30.0),
        ],
        closed: true,
    };
    assert!(toggle_corner_smooth(&mut closed, 0));
    assert!(
        closed.anchors[0].handle_in != (0.0, 0.0),
        "a closed start has a previous neighbour"
    );
    let mut lone = open(&[(5.0, 5.0)]);
    assert!(!toggle_corner_smooth(&mut lone, 0));
    assert!(!toggle_corner_smooth(&mut lone, 9), "out of range");
}

#[test]
fn reversed_swaps_handles_and_order() {
    let sub = Subpath {
        anchors: vec![smooth((0.0, 0.0), (5.0, 1.0)), Anchor::corner(9.0, 9.0)],
        closed: true,
    };
    let back = reversed(&sub);
    assert_eq!(back.anchors[1].point, (0.0, 0.0));
    assert_eq!(back.anchors[1].handle_out, (-5.0, -1.0));
    assert_eq!(back.anchors[1].handle_in, (5.0, 1.0));
    assert_eq!(reversed(&back), sub);
}

#[test]
fn insert_anchor_refuses_a_full_subpath() {
    let mut subs = vec![open(&vec![(0.0, 0.0); crate::MAX_ANCHORS_PER_SUBPATH])];
    let hit = SegmentHit {
        subpath: 0,
        segment: 0,
        t: 0.5,
        distance: 0.0,
    };
    assert_eq!(insert_anchor(&mut subs, &hit), None);
    assert_eq!(subs[0].anchors.len(), crate::MAX_ANCHORS_PER_SUBPATH);
}
