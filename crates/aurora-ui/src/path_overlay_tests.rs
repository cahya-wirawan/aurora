#![allow(
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap
)]

use std::collections::HashMap;

use aurora_doc::{Anchor, Path, Subpath};
use aurora_widgets::widgets::{OutlineRun, OutlineTone};

use super::{
    AnchorRef, MAX_DRAWN_ANCHORS, PathPart, path_anchor_screen, path_hit, path_overlay_runs,
};
use crate::CanvasView;

fn path(subpaths: Vec<Subpath>) -> Path {
    Path {
        name: "p".to_owned(),
        subpaths,
    }
}

fn corners(points: &[(f32, f32)], closed: bool) -> Subpath {
    Subpath {
        anchors: points.iter().map(|&(x, y)| Anchor::corner(x, y)).collect(),
        closed,
    }
}

fn at(subpath: usize, anchor: usize) -> AnchorRef {
    AnchorRef { subpath, anchor }
}

fn pixels(runs: &[OutlineRun]) -> HashMap<(i64, i64), Vec<OutlineTone>> {
    let mut map: HashMap<(i64, i64), Vec<OutlineTone>> = HashMap::new();
    for run in runs {
        for dy in 0..i64::from(run.height) {
            for dx in 0..i64::from(run.width) {
                map.entry((i64::from(run.x) + dx, i64::from(run.y) + dy))
                    .or_default()
                    .push(run.tone);
            }
        }
    }
    map
}

fn tone_at(map: &HashMap<(i64, i64), Vec<OutlineTone>>, x: i64, y: i64) -> Option<OutlineTone> {
    map.get(&(x, y)).and_then(|t| t.first().copied())
}

fn view() -> CanvasView {
    CanvasView::default()
}

const SIZE: (f32, f32) = (200.0, 120.0);

#[test]
fn an_empty_path_or_an_area_without_size_draws_nothing() {
    assert!(path_overlay_runs(&path(vec![]), None, &view(), SIZE, 1.0, 6).is_empty());
    assert!(
        path_overlay_runs(&path(vec![Subpath::default()]), None, &view(), SIZE, 1.0, 6).is_empty()
    );
    let p = path(vec![corners(&[(10.0, 10.0), (50.0, 10.0)], false)]);
    assert!(path_overlay_runs(&p, None, &view(), (0.0, 50.0), 1.0, 6).is_empty());
    assert!(!path_overlay_runs(&p, None, &view(), SIZE, 1.0, 6).is_empty());
}

#[test]
fn a_line_is_one_light_pixel_on_a_one_pixel_dark_rim() {
    let p = path(vec![corners(&[(10.0, 20.0), (80.0, 20.0)], false)]);
    let map = pixels(&path_overlay_runs(&p, None, &view(), SIZE, 1.0, 6));
    for x in [30_i64, 45, 60] {
        assert_eq!(
            tone_at(&map, x, 20),
            Some(OutlineTone::Light),
            "core at {x}"
        );
        assert_eq!(tone_at(&map, x, 19), Some(OutlineTone::Dark));
        assert_eq!(tone_at(&map, x, 21), Some(OutlineTone::Dark));
        assert_eq!(tone_at(&map, x, 18), None);
        assert_eq!(tone_at(&map, x, 22), None);
    }
    // No pixel is painted in two tones.
    assert!(map.values().all(|tones| tones.len() == 1));
}

#[test]
fn a_closed_subpath_includes_its_closing_segment_and_an_open_one_does_not() {
    let tri = [(20.0, 20.0), (100.0, 20.0), (100.0, 90.0)];
    let closed = pixels(&path_overlay_runs(
        &path(vec![corners(&tri, true)]),
        None,
        &view(),
        SIZE,
        1.0,
        6,
    ));
    let open = pixels(&path_overlay_runs(
        &path(vec![corners(&tri, false)]),
        None,
        &view(),
        SIZE,
        1.0,
        6,
    ));
    // The midpoint of the closing edge (100,90) -> (20,20) is (60, 55).
    let near = |map: &HashMap<(i64, i64), Vec<OutlineTone>>| {
        (-1..=1).any(|d| tone_at(map, 60 + d, 55) == Some(OutlineTone::Light))
            || (-1..=1).any(|d| tone_at(map, 60, 55 + d) == Some(OutlineTone::Light))
    };
    assert!(near(&closed), "the closing edge is drawn");
    assert!(!near(&open), "an open path is not closed on screen");
}

#[test]
fn a_curve_is_drawn_curved_through_its_flattening() {
    let mut a = Anchor::corner(20.0, 60.0);
    a.handle_out = (0.0, -50.0);
    let mut b = Anchor::corner(120.0, 60.0);
    b.handle_in = (0.0, -50.0);
    let p = path(vec![Subpath {
        anchors: vec![a, b],
        closed: false,
    }]);
    let map = pixels(&path_overlay_runs(&p, None, &view(), SIZE, 1.0, 6));
    // The cubic's apex is at t = 0.5: y = 60 - 0.75 * 50 = 22.5.
    let top = (0..120_i64)
        .filter(|&y| tone_at(&map, 70, y) == Some(OutlineTone::Light))
        .min();
    assert!(matches!(top, Some(y) if (21..=24).contains(&y)), "{top:?}");
    // And the straight chord at y = 60 is not drawn mid-way.
    assert_eq!(tone_at(&map, 70, 60), None);
}

#[test]
fn anchors_are_squares_with_a_rim_and_the_selected_one_is_accent_filled() {
    let p = path(vec![corners(&[(30.0, 40.0), (90.0, 40.0)], false)]);
    let runs = path_overlay_runs(&p, Some(at(0, 1)), &view(), SIZE, 1.0, 6);
    let map = pixels(&runs);
    // Anchor 0 at (30, 40): inner 27..33, rim one pixel round.
    for y in 37..43 {
        for x in 27..33 {
            assert_eq!(tone_at(&map, x, y), Some(OutlineTone::Light), "({x},{y})");
        }
    }
    assert_eq!(tone_at(&map, 26, 40), Some(OutlineTone::Dark));
    assert_eq!(tone_at(&map, 33, 40), Some(OutlineTone::Dark));
    assert_eq!(tone_at(&map, 30, 36), Some(OutlineTone::Dark));
    assert_eq!(tone_at(&map, 30, 43), Some(OutlineTone::Dark));
    // Anchor 1 at (90, 40) is selected: accent inside the same rim.
    for y in 37..43 {
        for x in 87..93 {
            assert_eq!(tone_at(&map, x, y), Some(OutlineTone::Accent), "({x},{y})");
        }
    }
    assert_eq!(tone_at(&map, 86, 40), Some(OutlineTone::Dark));
    // The line does not run through the squares' interior or rim as dark.
    assert!(
        map.values().all(|tones| tones.len() == 1),
        "tones are disjoint"
    );
    // A zero-sized square draws only the line.
    let none = path_overlay_runs(&p, Some(at(0, 1)), &view(), SIZE, 1.0, 0);
    assert!(none.iter().all(|r| r.tone != OutlineTone::Accent));
    assert!(!none.is_empty());
}

#[test]
fn only_the_selected_anchors_handles_are_drawn() {
    let mut mid = Anchor::corner(80.0, 60.0);
    mid.handle_in = (-30.0, 0.0);
    mid.handle_out = (30.0, 0.0);
    let mut other = Anchor::corner(150.0, 100.0);
    other.handle_out = (0.0, -20.0);
    let p = path(vec![Subpath {
        anchors: vec![
            Anchor {
                point: (10.0, 110.0),
                handle_in: (0.0, 0.0),
                handle_out: (0.0, 8.0),
            },
            mid,
            other,
        ],
        closed: false,
    }]);
    let selected = pixels(&path_overlay_runs(
        &p,
        Some(at(0, 1)),
        &view(),
        SIZE,
        1.0,
        6,
    ));
    // Handle ends of the selected anchor: squares at (50, 60) and (110, 60).
    assert_eq!(tone_at(&selected, 50, 60), Some(OutlineTone::Light));
    assert_eq!(tone_at(&selected, 110, 60), Some(OutlineTone::Light));
    // The handle line between the anchor and its end.
    assert_eq!(tone_at(&selected, 95, 60), Some(OutlineTone::Light));
    assert_eq!(tone_at(&selected, 65, 60), Some(OutlineTone::Light));
    // The other anchor's handle (150,100)->(150,80) is not drawn.
    assert_eq!(tone_at(&selected, 150, 90), None);
    let unselected = pixels(&path_overlay_runs(&p, None, &view(), SIZE, 1.0, 6));
    assert_eq!(tone_at(&unselected, 95, 60), None);
    assert_eq!(tone_at(&unselected, 50, 60), None);
    // The first anchor's own handle end shows only when it is selected.
    assert_eq!(tone_at(&unselected, 10, 118), None);
    assert_eq!(tone_at(&selected, 10, 118), None);
    let first = pixels(&path_overlay_runs(
        &p,
        Some(at(0, 0)),
        &view(),
        SIZE,
        1.0,
        6,
    ));
    assert_eq!(tone_at(&first, 10, 118), Some(OutlineTone::Light));
}

#[test]
fn scale_two_doubles_positions_but_not_the_square_in_physical_pixels() {
    let p = path(vec![corners(&[(30.0, 40.0), (90.0, 40.0)], false)]);
    let map = pixels(&path_overlay_runs(&p, None, &view(), SIZE, 2.0, 12));
    assert_eq!(
        tone_at(&map, 60, 80),
        Some(OutlineTone::Light),
        "anchor centre"
    );
    assert_eq!(tone_at(&map, 54, 80), Some(OutlineTone::Light));
    assert_eq!(tone_at(&map, 53, 80), Some(OutlineTone::Dark));
    assert_eq!(
        tone_at(&map, 120, 80),
        Some(OutlineTone::Light),
        "on the line"
    );
    assert_eq!(tone_at(&map, 120, 79), Some(OutlineTone::Dark));
    // The canvas area is 400 x 240 physical pixels.
    assert!(
        map.keys()
            .all(|&(x, y)| (0..400).contains(&x) && (0..240).contains(&y))
    );
}

#[test]
fn a_zoomed_and_panned_view_moves_the_overlay_with_the_document() {
    let mut v = view();
    v.zoom_at((0.0, 0.0), 2.0);
    v.pan_by((15.0, 5.0));
    let p = path(vec![corners(&[(10.0, 10.0), (40.0, 10.0)], false)]);
    let (sx, sy) = v.to_screen((10.0, 10.0));
    let map = pixels(&path_overlay_runs(&p, None, &v, SIZE, 1.0, 6));
    let (x, y) = (sx.round() as i64, sy.round() as i64);
    assert_eq!(tone_at(&map, x, y), Some(OutlineTone::Light));
    assert_eq!(tone_at(&map, x + 20, y), Some(OutlineTone::Light));
    assert_eq!(tone_at(&map, x + 20, y + 1), Some(OutlineTone::Dark));
}

#[test]
fn everything_is_clipped_to_the_canvas_area_and_far_paths_vanish() {
    let p = path(vec![corners(&[(-500.0, 50.0), (900.0, 60.0)], false)]);
    let runs = path_overlay_runs(&p, None, &view(), SIZE, 1.0, 6);
    assert!(!runs.is_empty());
    for r in &runs {
        assert!(r.x >= 0 && r.y >= 0);
        assert!(i64::from(r.x) + i64::from(r.width) <= 200);
        assert!(i64::from(r.y) + i64::from(r.height) <= 120);
    }
    let far = path(vec![corners(&[(1.0e6, 1.0e6), (2.0e6, 1.5e6)], false)]);
    assert!(path_overlay_runs(&far, None, &view(), SIZE, 1.0, 6).is_empty());
    // A square half outside is cut, not dropped.
    let edge = path(vec![corners(&[(0.0, 50.0), (60.0, 50.0)], false)]);
    let map = pixels(&path_overlay_runs(&edge, None, &view(), SIZE, 1.0, 6));
    assert_eq!(tone_at(&map, 0, 50), Some(OutlineTone::Light));
    assert!(map.keys().all(|&(x, _)| x >= 0));
}

#[test]
fn a_single_anchor_shows_its_square_and_a_crowd_is_bounded() {
    let p = path(vec![corners(&[(50.0, 50.0)], false)]);
    let map = pixels(&path_overlay_runs(&p, None, &view(), SIZE, 1.0, 6));
    assert_eq!(tone_at(&map, 50, 50), Some(OutlineTone::Light));
    let many: Vec<(f32, f32)> = (0..9000)
        .map(|i| ((i % 190) as f32, (i / 190 % 110) as f32))
        .collect();
    let crowd = path(vec![corners(&many, false)]);
    let runs = path_overlay_runs(&crowd, None, &view(), SIZE, 1.0, 6);
    let squares = runs
        .iter()
        .filter(|r| r.width == 6 && r.height == 6)
        .count();
    assert!(squares <= MAX_DRAWN_ANCHORS);
}

#[test]
fn hits_find_anchors_within_the_radius_and_the_last_drawn_wins() {
    let p = path(vec![
        corners(&[(50.0, 50.0), (100.0, 50.0)], false),
        corners(&[(52.0, 50.0)], false),
    ]);
    let v = view();
    assert_eq!(
        path_hit(&p, None, &v, (100.0, 54.0), 12.0),
        Some(PathPart::Anchor(at(0, 1)))
    );
    assert_eq!(
        path_hit(&p, None, &v, (100.0, 63.0), 12.0),
        None,
        "outside the radius"
    );
    assert_eq!(
        path_hit(&p, None, &v, (100.0, 62.0), 12.0),
        Some(PathPart::Anchor(at(0, 1))),
        "on the radius"
    );
    // Two anchors in reach: the later subpath's wins.
    assert_eq!(
        path_hit(&p, None, &v, (51.0, 50.0), 12.0),
        Some(PathPart::Anchor(at(1, 0)))
    );
    assert_eq!(path_hit(&p, None, &v, (f32::NAN, 0.0), 12.0), None);
    assert_eq!(path_hit(&p, None, &v, (50.0, 50.0), -1.0), None);
}

#[test]
fn a_selected_anchors_handles_win_over_anchors_and_unselected_ones_are_not_hit() {
    let mut a = Anchor::corner(80.0, 60.0);
    a.handle_in = (-5.0, 0.0);
    a.handle_out = (40.0, 0.0);
    let near = Anchor::corner(122.0, 60.0);
    let p = path(vec![Subpath {
        anchors: vec![a, near],
        closed: false,
    }]);
    let v = view();
    // The handle end (120, 60) is within reach of both it and the anchor
    // at (122, 60): the handle wins when its anchor is selected.
    assert_eq!(
        path_hit(&p, Some(at(0, 0)), &v, (121.0, 60.0), 8.0),
        Some(PathPart::HandleOut(at(0, 0)))
    );
    assert_eq!(
        path_hit(&p, None, &v, (121.0, 60.0), 8.0),
        Some(PathPart::Anchor(at(0, 1)))
    );
    // A zero handle is never hit; the incoming one is.
    assert_eq!(
        path_hit(&p, Some(at(0, 1)), &v, (127.0, 60.0), 3.0),
        None,
        "a corner has no handles"
    );
    assert_eq!(
        path_hit(&p, Some(at(0, 0)), &v, (75.0, 61.0), 3.0),
        Some(PathPart::HandleIn(at(0, 0)))
    );
}

#[test]
fn hit_testing_follows_the_view() {
    let mut v = view();
    v.zoom_at((0.0, 0.0), 4.0);
    v.pan_by((-20.0, 10.0));
    let p = path(vec![corners(&[(10.0, 10.0)], false)]);
    let screen = path_anchor_screen(&p, at(0, 0), &v);
    assert_eq!(screen, Some(v.to_screen((10.0, 10.0))));
    let (sx, sy) = v.to_screen((10.0, 10.0));
    assert_eq!(
        path_hit(&p, None, &v, (sx + 5.0, sy), 8.0),
        Some(PathPart::Anchor(at(0, 0)))
    );
    assert_eq!(path_hit(&p, None, &v, (sx + 5.0, sy), 4.0), None);
    assert_eq!(path_anchor_screen(&p, at(3, 0), &v), None);
}
