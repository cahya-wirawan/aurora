//! Tests of the Pen tool's state machine (0.233.0): clicks, drags, closing,
//! ending, cancelling, anchor and handle moves, deletion, caps and steps.

#![allow(
    clippy::cast_precision_loss,
    clippy::indexing_slicing,
    clippy::redundant_closure_for_method_calls
)]

use aurora_doc::{Anchor, MAX_ANCHORS_PER_SUBPATH, MAX_SUBPATHS, PathSet, Subpath, WORK_PATH_NAME};
use aurora_ui::{AnchorRef, PathPart};

use super::{
    ADD_POINT_LABEL, CANCEL_SUBPATH_LABEL, CLOSE_PATH_LABEL, DELETE_ANCHOR_LABEL,
    MOVE_ANCHOR_LABEL, MOVE_HANDLE_LABEL, PenOutcome, PenState, REMOVE_POINT_LABEL,
};

const R: f32 = 5.0;

fn r(subpath: usize, anchor: usize) -> AnchorRef {
    AnchorRef { subpath, anchor }
}

fn click(pen: &mut PenState, paths: &mut PathSet, p: (f32, f32)) -> PenOutcome {
    let down = pen.press(paths, p, None, R, false);
    assert!(down.refused.is_none());
    let up = pen.release(paths);
    PenOutcome {
        step: up.step,
        refused: up.refused,
        redraw: true,
    }
}

fn work(paths: &PathSet) -> Vec<Subpath> {
    paths.work().map(|p| p.subpaths.clone()).unwrap_or_default()
}

fn points(sub: &Subpath) -> Vec<(f32, f32)> {
    sub.anchors.iter().map(|a| a.point).collect()
}

#[test]
fn the_first_click_creates_the_work_path_and_each_click_is_one_step() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    let first = click(&mut pen, &mut paths, (10.0, 10.0));
    let Some(step) = first.step else {
        unreachable!("a step");
    };
    assert_eq!(step.label, ADD_POINT_LABEL);
    assert!(step.before.is_empty(), "the work path is part of the step");
    assert_eq!(step.after, paths);
    assert_eq!(pen.active(), Some(WORK_PATH_NAME));
    let second = click(&mut pen, &mut paths, (30.0, 10.0));
    let Some(step) = second.step else {
        unreachable!("a step");
    };
    assert_eq!(step.before.work().map(|p| p.anchor_count()), Some(1));
    assert_eq!(step.after.work().map(|p| p.anchor_count()), Some(2));
    let subs = work(&paths);
    assert_eq!(subs.len(), 1);
    assert_eq!(points(&subs[0]), vec![(10.0, 10.0), (30.0, 10.0)]);
    assert!(subs[0].anchors.iter().all(Anchor::is_corner));
    assert!(!subs[0].closed);
    assert_eq!(pen.selected(), Some(r(0, 1)));
}

#[test]
fn a_drag_makes_a_smooth_anchor_with_mirrored_handles() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    assert!(
        pen.press(&mut paths, (20.0, 20.0), None, R, false)
            .step
            .is_none()
    );
    assert!(pen.dragging());
    assert!(pen.drag(&mut paths, (30.0, 15.0)));
    let live = work(&paths);
    assert_eq!(live[0].anchors[0].handle_out, (10.0, -5.0));
    assert_eq!(live[0].anchors[0].handle_in, (-10.0, 5.0));
    // Still dragging: the handle follows the pointer.
    assert!(pen.drag(&mut paths, (20.0, 40.0)));
    assert_eq!(work(&paths)[0].anchors[0].handle_out, (0.0, 20.0));
    let up = pen.release(&mut paths);
    assert_eq!(up.step.map(|s| s.label), Some(ADD_POINT_LABEL));
    assert!(!pen.dragging());
    assert!(!pen.drag(&mut paths, (1.0, 1.0)), "no gesture, no drag");
}

#[test]
fn pressing_the_first_anchor_closes_the_subpath() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    for p in [(0.0, 0.0), (40.0, 0.0), (40.0, 40.0)] {
        click(&mut pen, &mut paths, p);
    }
    // Outside the tolerance: another anchor.
    let far = click(&mut pen, &mut paths, (0.0, 9.0));
    assert_eq!(far.step.map(|s| s.label), Some(ADD_POINT_LABEL));
    assert_eq!(work(&paths)[0].anchors.len(), 4);
    assert!(!work(&paths)[0].closed);
    // Inside it: closed, one step, selected is the first anchor.
    let out = pen.press(&mut paths, (2.0, 3.0), None, R, false);
    let Some(step) = out.step else {
        unreachable!("closing is a step");
    };
    assert_eq!(step.label, CLOSE_PATH_LABEL);
    assert!(work(&paths)[0].closed);
    assert_eq!(work(&paths)[0].anchors.len(), 4, "no anchor added");
    assert_eq!(pen.drawing(), None);
    assert_eq!(pen.selected(), Some(r(0, 0)));
    assert!(!pen.dragging());
    // The next click starts a new subpath.
    click(&mut pen, &mut paths, (100.0, 100.0));
    assert_eq!(work(&paths).len(), 2);
    assert_eq!(pen.drawing(), Some(1));
}

#[test]
fn one_anchor_cannot_be_closed_on_itself() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    click(&mut pen, &mut paths, (5.0, 5.0));
    click(&mut pen, &mut paths, (5.0, 5.0));
    assert!(!work(&paths)[0].closed);
    assert_eq!(work(&paths)[0].anchors.len(), 2);
}

#[test]
fn enter_ends_an_open_subpath_without_a_step() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    click(&mut pen, &mut paths, (0.0, 0.0));
    click(&mut pen, &mut paths, (9.0, 0.0));
    assert!(pen.end_subpath());
    assert!(!pen.end_subpath(), "nothing left to end");
    click(&mut pen, &mut paths, (20.0, 20.0));
    let subs = work(&paths);
    assert_eq!(subs.len(), 2);
    assert!(!subs[0].closed, "ended open");
    assert_eq!(subs[1].anchors.len(), 1);
}

#[test]
fn escape_removes_only_the_subpath_in_progress_as_one_step() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    for p in [(0.0, 0.0), (9.0, 0.0), (9.0, 9.0)] {
        click(&mut pen, &mut paths, p);
    }
    pen.end_subpath();
    click(&mut pen, &mut paths, (50.0, 50.0));
    click(&mut pen, &mut paths, (60.0, 50.0));
    let out = pen.cancel_subpath(&mut paths);
    let Some(step) = out.step else {
        unreachable!("a step");
    };
    assert_eq!(step.label, CANCEL_SUBPATH_LABEL);
    assert_eq!(work(&paths).len(), 1, "the finished subpath stays");
    assert_eq!(work(&paths)[0].anchors.len(), 3);
    // Nothing in progress now: no step.
    assert!(pen.cancel_subpath(&mut paths).step.is_none());
}

#[test]
fn backspace_removes_the_last_point_and_the_empty_subpath() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    click(&mut pen, &mut paths, (0.0, 0.0));
    click(&mut pen, &mut paths, (9.0, 0.0));
    let out = pen.remove_last_point(&mut paths);
    assert_eq!(out.step.as_ref().map(|s| s.label), Some(REMOVE_POINT_LABEL));
    assert_eq!(points(&work(&paths)[0]), vec![(0.0, 0.0)]);
    assert_eq!(pen.selected(), Some(r(0, 0)));
    let out = pen.remove_last_point(&mut paths);
    assert!(out.step.is_some());
    assert!(work(&paths).is_empty(), "an empty subpath goes");
    assert_eq!(pen.drawing(), None);
    assert!(pen.remove_last_point(&mut paths).step.is_none());
}

fn with_square() -> (PenState, PathSet) {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    for p in [(0.0, 0.0), (40.0, 0.0), (40.0, 40.0)] {
        click(&mut pen, &mut paths, p);
    }
    pen.press(&mut paths, (1.0, 1.0), None, R, false);
    (pen, paths)
}

#[test]
fn dragging_an_anchor_moves_it_with_its_handles_and_is_one_step() {
    let (mut pen, mut paths) = with_square();
    // Give anchor 1 handles first.
    if let Some(subs) = paths.subpaths_mut(WORK_PATH_NAME) {
        subs[0].anchors[1] = Anchor::smooth((40.0, 0.0), (6.0, 2.0));
    }
    let before = paths.clone();
    let down = pen.press(
        &mut paths,
        (41.0, 1.0),
        Some(PathPart::Anchor(r(0, 1))),
        R,
        false,
    );
    assert!(down.step.is_none());
    assert_eq!(pen.selected(), Some(r(0, 1)));
    assert!(pen.drag(&mut paths, (51.0, 11.0)));
    let a = work(&paths)[0].anchors[1];
    assert_eq!(a.point, (50.0, 10.0), "the grab offset is kept");
    assert_eq!(a.handle_out, (6.0, 2.0), "handles are offsets: they travel");
    let up = pen.release(&mut paths);
    let Some(step) = up.step else {
        unreachable!("moved");
    };
    assert_eq!(step.label, MOVE_ANCHOR_LABEL);
    assert_eq!(step.before, before);
    assert_eq!(step.after, paths);
}

#[test]
fn a_press_on_an_anchor_without_a_move_records_nothing() {
    let (mut pen, mut paths) = with_square();
    let before = paths.clone();
    pen.press(
        &mut paths,
        (0.0, 0.0),
        Some(PathPart::Anchor(r(0, 0))),
        0.0,
        false,
    );
    let up = pen.release(&mut paths);
    assert!(up.step.is_none());
    assert_eq!(paths, before);
    assert_eq!(pen.selected(), Some(r(0, 0)), "but it is selected");
}

#[test]
fn a_handle_of_a_smooth_anchor_mirrors_and_alt_breaks_it() {
    for (alt, mirrored) in [(false, true), (true, false)] {
        let (mut pen, mut paths) = with_square();
        if let Some(subs) = paths.subpaths_mut(WORK_PATH_NAME) {
            subs[0].anchors[1] = Anchor::smooth((40.0, 0.0), (6.0, 2.0));
        }
        pen.press(
            &mut paths,
            (46.0, 2.0),
            Some(PathPart::HandleOut(r(0, 1))),
            R,
            alt,
        );
        assert!(pen.drag(&mut paths, (50.0, 10.0)));
        let a = work(&paths)[0].anchors[1];
        assert_eq!(a.handle_out, (10.0, 10.0));
        if mirrored {
            assert_eq!(a.handle_in, (-10.0, -10.0));
        } else {
            assert_eq!(a.handle_in, (-6.0, -2.0), "alt leaves the other handle");
        }
        let up = pen.release(&mut paths);
        assert_eq!(up.step.map(|s| s.label), Some(MOVE_HANDLE_LABEL));
    }
}

#[test]
fn the_incoming_handle_drags_too_and_a_corner_with_one_handle_does_not_mirror() {
    let (mut pen, mut paths) = with_square();
    if let Some(subs) = paths.subpaths_mut(WORK_PATH_NAME) {
        subs[0].anchors[1] = Anchor {
            point: (40.0, 0.0),
            handle_in: (-5.0, 0.0),
            handle_out: (0.0, 0.0),
        };
    }
    pen.press(
        &mut paths,
        (35.0, 0.0),
        Some(PathPart::HandleIn(r(0, 1))),
        R,
        false,
    );
    pen.drag(&mut paths, (30.0, 4.0));
    let a = work(&paths)[0].anchors[1];
    assert_eq!(a.handle_in, (-10.0, 4.0));
    assert_eq!(
        a.handle_out,
        (0.0, 0.0),
        "no mirror for a one-handle corner"
    );
    pen.release(&mut paths);
}

#[test]
fn delete_removes_the_selected_anchor_and_unselects() {
    let (mut pen, mut paths) = with_square();
    assert!(pen.delete_selected(&mut PathSet::new()).step.is_none());
    pen.press(
        &mut paths,
        (40.0, 0.0),
        Some(PathPart::Anchor(r(0, 1))),
        R,
        false,
    );
    pen.release(&mut paths);
    let out = pen.delete_selected(&mut paths);
    assert_eq!(
        out.step.as_ref().map(|s| s.label),
        Some(DELETE_ANCHOR_LABEL)
    );
    assert_eq!(points(&work(&paths)[0]), vec![(0.0, 0.0), (40.0, 40.0)]);
    assert_eq!(pen.selected(), None);
    assert!(
        pen.delete_selected(&mut paths).step.is_none(),
        "nothing selected"
    );
}

#[test]
fn deleting_down_to_one_anchor_opens_a_closed_subpath_and_to_none_removes_it() {
    let (mut pen, mut paths) = with_square();
    // `with_square` closed it; delete two anchors.
    assert!(work(&paths)[0].closed);
    for _ in 0..2 {
        pen.press(
            &mut paths,
            (0.0, 0.0),
            Some(PathPart::Anchor(r(0, 0))),
            R,
            false,
        );
        pen.release(&mut paths);
        pen.delete_selected(&mut paths);
    }
    assert_eq!(work(&paths)[0].anchors.len(), 1);
    assert!(!work(&paths)[0].closed, "a lone anchor cannot be closed");
    pen.press(
        &mut paths,
        (0.0, 0.0),
        Some(PathPart::Anchor(r(0, 0))),
        R,
        false,
    );
    pen.release(&mut paths);
    pen.delete_selected(&mut paths);
    assert!(work(&paths).is_empty());
}

#[test]
fn the_anchor_and_subpath_caps_refuse_without_a_step() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    click(&mut pen, &mut paths, (1.0, 1.0));
    if let Some(subs) = paths.subpaths_mut(WORK_PATH_NAME) {
        subs[0].anchors = vec![Anchor::corner(1.0, 1.0); MAX_ANCHORS_PER_SUBPATH];
    }
    let before = paths.clone();
    let out = pen.press(&mut paths, (50.0, 50.0), None, 0.0, false);
    assert!(out.refused.is_some());
    assert_eq!(paths, before);
    assert!(!pen.dragging());
    // Subpaths: end the open one each time.
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    for i in 0..MAX_SUBPATHS {
        click(&mut pen, &mut paths, (i as f32, 1.0));
        pen.end_subpath();
    }
    let before = paths.clone();
    let out = pen.press(&mut paths, (9.0, 9.0), None, 0.0, false);
    assert!(out.refused.is_some());
    assert_eq!(paths, before);
}

#[test]
fn a_full_document_refuses_a_new_work_path() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    for i in 0..aurora_doc::MAX_PATHS {
        assert!(paths.add(&format!("p{i}")).is_ok());
    }
    let out = pen.press(&mut paths, (1.0, 1.0), None, R, false);
    assert!(out.refused.is_some());
    assert!(paths.work().is_none());
    assert!(pen.release(&mut paths).step.is_none());
}

#[test]
fn a_coordinate_out_of_range_restores_the_path_and_says_so() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    click(&mut pen, &mut paths, (1.0, 1.0));
    let before = paths.clone();
    pen.press(&mut paths, (2.0, 2.0), None, 0.0, false);
    pen.drag(&mut paths, (2.0e7, 0.0));
    let up = pen.release(&mut paths);
    assert!(up.refused.is_some());
    assert!(up.step.is_none());
    assert_eq!(paths, before, "the half-made anchor is gone too");
}

#[test]
fn a_named_path_is_drawn_on_and_switching_forgets_the_subpath() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    assert!(paths.add("Outline").is_ok());
    pen.set_active(Some("Outline".to_owned()));
    click(&mut pen, &mut paths, (3.0, 3.0));
    assert!(paths.work().is_none(), "no work path when a path is active");
    assert_eq!(paths.get("Outline").map(|p| p.anchor_count()), Some(1));
    assert_eq!(pen.drawing(), Some(0));
    pen.set_active(Some("Outline".to_owned()));
    assert_eq!(pen.drawing(), Some(0), "the same path keeps its subpath");
    assert!(paths.add("Other").is_ok());
    pen.set_active(Some("Other".to_owned()));
    assert_eq!(pen.drawing(), None);
    assert_eq!(pen.selected(), None);
    // An undo that removed the active path leaves nothing active.
    pen.paths_replaced(&PathSet::new());
    assert_eq!(pen.active(), None);
}

#[test]
fn another_document_clears_the_state() {
    use crate::document_session::DocumentId;
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    click(&mut pen, &mut paths, (3.0, 3.0));
    let a = DocumentId::next();
    assert!(pen.adopt_document(a), "a different document: cleared");
    assert_eq!(pen.active(), None);
    assert!(!pen.adopt_document(a), "the same one is left alone");
}

#[test]
fn the_document_wide_point_limit_is_enforced_at_the_press() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    assert!(paths.add("big").is_ok());
    let per = MAX_ANCHORS_PER_SUBPATH;
    let full = aurora_doc::MAX_PATH_ANCHORS_TOTAL / per;
    let subs: Vec<Subpath> = (0..full)
        .map(|_| Subpath {
            anchors: vec![Anchor::corner(1.0, 1.0); per],
            closed: false,
        })
        .collect();
    assert!(paths.set_subpaths("big", subs).is_ok());
    assert!(paths.add("other").is_ok());
    pen.set_active(Some("other".to_owned()));
    let before = paths.clone();
    let out = pen.press(&mut paths, (5.0, 5.0), None, 0.0, false);
    assert_eq!(out.refused, Some(super::POINT_LIMIT_NOTICE));
    assert_eq!(paths, before);
    assert!(!pen.dragging());
    assert!(pen.release(&mut paths).step.is_none());
}

#[test]
fn a_release_names_a_range_violation_and_a_cap_violation_differently() {
    use aurora_doc::DocError;
    assert_eq!(
        super::release_notice(&DocError::InvalidPath("a coordinate is not usable")),
        super::RANGE_NOTICE
    );
    assert_eq!(
        super::release_notice(&DocError::InvalidPath("too many anchors in the document")),
        super::POINT_LIMIT_NOTICE
    );
    // Through a real release: range.
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    click(&mut pen, &mut paths, (1.0, 1.0));
    pen.press(&mut paths, (2.0, 2.0), None, 0.0, false);
    pen.drag(&mut paths, (2.0e7, 0.0));
    assert_eq!(pen.release(&mut paths).refused, Some(super::RANGE_NOTICE));
    // And a cap: the set grows past the limit during the gesture.
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    pen.press(&mut paths, (2.0, 2.0), None, 0.0, false);
    if let Some(subs) = paths.subpaths_mut(WORK_PATH_NAME) {
        for _ in 0..=(aurora_doc::MAX_PATH_ANCHORS_TOTAL / MAX_ANCHORS_PER_SUBPATH) {
            subs.push(Subpath {
                anchors: vec![Anchor::corner(1.0, 1.0); MAX_ANCHORS_PER_SUBPATH],
                closed: false,
            });
        }
    }
    let up = pen.release(&mut paths);
    assert_eq!(up.refused, Some(super::POINT_LIMIT_NOTICE));
}

// ---- 0.234.0: pen editing ------------------------------------------------

use super::{CONVERT_POINT_LABEL, INSERT_POINT_LABEL};
use aurora_doc::SegmentHit;

fn open_line() -> (PenState, PathSet) {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    for p in [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)] {
        click(&mut pen, &mut paths, p);
    }
    pen.end_subpath();
    (pen, paths)
}

#[test]
fn a_press_on_a_segment_inserts_an_anchor_as_one_add_point_step() {
    let (mut pen, mut paths) = open_line();
    let hit = SegmentHit {
        subpath: 0,
        segment: 0,
        t: 0.25,
        distance: 1.0,
    };
    let down = pen.press_at(&mut paths, (25.0, 1.0), None, Some(hit), R, false);
    let Some(step) = down.step else {
        unreachable!("a step");
    };
    assert_eq!(step.label, INSERT_POINT_LABEL);
    assert_eq!(step.label, "Add Point");
    assert!(!pen.dragging(), "no drag follows an insertion");
    let subs = work(&paths);
    assert_eq!(
        points(&subs[0]),
        vec![(0.0, 0.0), (25.0, 0.0), (100.0, 0.0), (100.0, 100.0)]
    );
    assert_eq!(pen.selected(), Some(r(0, 1)));
    assert_eq!(step.before.work().map(|p| p.anchor_count()), Some(3));
    assert_eq!(step.after.work().map(|p| p.anchor_count()), Some(4));
}

#[test]
fn a_segment_press_while_a_subpath_is_in_progress_adds_at_the_pointer_instead() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    click(&mut pen, &mut paths, (0.0, 0.0));
    click(&mut pen, &mut paths, (100.0, 0.0));
    let hit = SegmentHit {
        subpath: 0,
        segment: 0,
        t: 0.5,
        distance: 1.0,
    };
    let down = pen.press_at(&mut paths, (50.0, 1.0), None, Some(hit), R, false);
    pen.release(&mut paths);
    assert_eq!(
        down.step.map(|s| s.label),
        None,
        "the add is recorded at the release"
    );
    assert_eq!(work(&paths)[0].anchors.len(), 3);
    assert_eq!(work(&paths)[0].anchors[2].point, (50.0, 1.0));
}

#[test]
fn an_anchor_hit_wins_over_a_segment() {
    let (mut pen, mut paths) = open_line();
    let hit = SegmentHit {
        subpath: 0,
        segment: 0,
        t: 0.0,
        distance: 0.0,
    };
    pen.press_at(
        &mut paths,
        (0.0, 0.0),
        Some(PathPart::Anchor(r(0, 0))),
        Some(hit),
        R,
        false,
    );
    assert!(pen.dragging());
    assert_eq!(work(&paths)[0].anchors.len(), 3, "nothing inserted");
}

#[test]
fn alt_click_on_an_anchor_converts_it_both_ways_as_one_step_each() {
    let (mut pen, mut paths) = open_line();
    let down = pen.press(
        &mut paths,
        (100.0, 0.0),
        Some(PathPart::Anchor(r(0, 1))),
        R,
        true,
    );
    let Some(step) = down.step else {
        unreachable!("a step");
    };
    assert_eq!(step.label, CONVERT_POINT_LABEL);
    assert!(!pen.dragging());
    let smooth = work(&paths)[0].anchors[1];
    assert!(!smooth.is_corner());
    assert!(smooth.handle_out.0 != 0.0 || smooth.handle_out.1 != 0.0);
    let back = pen.press(
        &mut paths,
        (100.0, 0.0),
        Some(PathPart::Anchor(r(0, 1))),
        R,
        true,
    );
    assert_eq!(back.step.map(|s| s.label), Some("Convert Point"));
    assert!(work(&paths)[0].anchors[1].is_corner());
}

#[test]
fn alt_click_on_a_lone_anchor_changes_nothing_and_records_nothing() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    click(&mut pen, &mut paths, (5.0, 5.0));
    let down = pen.press(
        &mut paths,
        (5.0, 5.0),
        Some(PathPart::Anchor(r(0, 0))),
        R,
        true,
    );
    assert!(down.step.is_none());
}

#[test]
fn delete_anchor_removes_a_chosen_anchor_without_a_selection() {
    let (mut pen, mut paths) = open_line();
    pen.reset_editing();
    assert_eq!(pen.selected(), None);
    let out = pen.delete_anchor(&mut paths, r(0, 1));
    assert_eq!(out.step.map(|s| s.label), Some(DELETE_ANCHOR_LABEL));
    assert_eq!(points(&work(&paths)[0]), vec![(0.0, 0.0), (100.0, 100.0)]);
}

#[test]
fn after_an_undo_the_next_click_continues_an_open_last_subpath_unless_enter_ended_it() {
    let mut pen = PenState::default();
    let mut paths = PathSet::new();
    click(&mut pen, &mut paths, (0.0, 0.0));
    click(&mut pen, &mut paths, (10.0, 0.0));
    // An undo replaces the path set from outside.
    pen.paths_replaced(&paths);
    assert!(pen.drawing().is_none());
    click(&mut pen, &mut paths, (20.0, 0.0));
    assert_eq!(work(&paths).len(), 1, "continued, not a new subpath");
    assert_eq!(work(&paths)[0].anchors.len(), 3);
    // Enter ends it: the next click starts a new subpath, even after the
    // tool was switched away and back (reset_editing keeps the flag).
    pen.end_subpath();
    pen.reset_editing();
    click(&mut pen, &mut paths, (50.0, 50.0));
    assert_eq!(work(&paths).len(), 2);
    // Switching path forgets the flag: with a closed last subpath nothing
    // is continued, with an open one it is.
    pen.set_active(None);
    pen.set_active(Some(WORK_PATH_NAME.to_owned()));
    click(&mut pen, &mut paths, (60.0, 60.0));
    assert_eq!(work(&paths).len(), 2);
    assert_eq!(work(&paths)[1].anchors.len(), 2);
}

#[test]
fn a_closed_last_subpath_is_never_continued() {
    let (mut pen, mut paths) = with_square();
    pen.paths_replaced(&paths);
    click(&mut pen, &mut paths, (500.0, 500.0));
    assert_eq!(work(&paths).len(), 2);
}
