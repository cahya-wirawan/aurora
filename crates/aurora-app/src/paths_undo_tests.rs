//! Path steps in the unified undo order (0.233.0): undo and redo exactness,
//! dirty neutrality, redo clearing, History labels and the Crop's shift.

use aurora_doc::{Anchor, PathSet, Subpath};

use crate::pen::PathStep;
use crate::{UndoDirection, UndoKind, UndoOrder};

fn set_with(x: f32) -> PathSet {
    let mut set = PathSet::new();
    assert!(set.add("A").is_ok());
    assert!(
        set.set_subpaths(
            "A",
            vec![Subpath {
                anchors: vec![Anchor::corner(x, 2.0), Anchor::corner(x + 5.0, 9.0)],
                closed: false,
            }],
        )
        .is_ok()
    );
    set
}

fn record(order: &mut UndoOrder, label: &str, before: PathSet, after: PathSet) -> bool {
    order.record_path_step(
        PathStep { before, after },
        label,
        &mut aurora_doc::History::new(),
        &mut aurora_brush::PixelHistory::default(),
    )
}

#[test]
fn undo_and_redo_put_back_exactly_the_recorded_sets() {
    let (a, b, c) = (PathSet::new(), set_with(1.0), set_with(7.5));
    let mut order = UndoOrder::default();
    assert!(record(&mut order, "Pen: Add Point", a.clone(), b.clone()));
    assert!(record(&mut order, "Move Anchor", b.clone(), c.clone()));
    assert_eq!(order.undo, vec![UndoKind::Path, UndoKind::Path]);
    assert_eq!(order.undo_labels, vec!["Pen: Add Point", "Move Anchor"]);
    assert!(order.apply_path_step(UndoDirection::Undo));
    assert_eq!(order.take_pending_paths(), Some(b.clone()));
    assert_eq!(order.take_pending_paths(), None, "taken once");
    assert!(order.apply_path_step(UndoDirection::Undo));
    assert_eq!(order.take_pending_paths(), Some(a));
    assert!(!order.apply_path_step(UndoDirection::Undo), "nothing left");
    assert!(order.apply_path_step(UndoDirection::Redo));
    assert_eq!(order.take_pending_paths(), Some(b));
    assert!(order.apply_path_step(UndoDirection::Redo));
    assert_eq!(order.take_pending_paths(), Some(c));
    assert!(!order.apply_path_step(UndoDirection::Redo));
    assert_eq!(order.undo.len(), 2);
}

#[test]
fn a_step_that_changed_nothing_is_not_recorded() {
    let mut order = UndoOrder::default();
    assert!(!record(&mut order, "x", set_with(1.0), set_with(1.0)));
    assert!(order.undo.is_empty());
}

#[test]
fn path_steps_never_dirty_the_document_but_move_the_revision() {
    let mut order = UndoOrder::default();
    let content = order.content_revision;
    let revision = order.revision;
    record(&mut order, "Pen: Add Point", PathSet::new(), set_with(1.0));
    assert_eq!(order.content_revision, content, "dirty-neutral");
    assert_ne!(order.revision, revision, "but autosaved");
    let revision = order.revision;
    assert!(order.apply_path_step(UndoDirection::Undo));
    assert_eq!(order.content_revision, content);
    assert_ne!(order.revision, revision);
    assert!(order.apply_path_step(UndoDirection::Redo));
    assert_eq!(order.content_revision, content);
}

#[test]
fn a_new_step_clears_the_redo_side_of_every_kind() {
    let mut order = UndoOrder::default();
    record(&mut order, "one", PathSet::new(), set_with(1.0));
    record(&mut order, "two", set_with(1.0), set_with(2.0));
    assert!(order.apply_path_step(UndoDirection::Undo));
    assert_eq!(order.path_redo.len(), 1);
    record(&mut order, "three", set_with(1.0), set_with(3.0));
    assert!(order.path_redo.is_empty());
    assert!(order.redo.is_empty());
    assert!(!order.apply_path_step(UndoDirection::Redo));
}

#[test]
fn a_path_undo_does_not_touch_a_step_of_another_kind() {
    let mut order = UndoOrder::default();
    order.undo.push(UndoKind::Pixel);
    assert!(!order.apply_path_step(UndoDirection::Undo));
    order.redo.push(UndoKind::Selection);
    assert!(!order.apply_path_step(UndoDirection::Redo));
    // The tag with no data behind it is refused, leaving both untouched.
    order.undo.push(UndoKind::Path);
    assert!(!order.apply_path_step(UndoDirection::Undo));
    assert_eq!(order.undo.len(), 2);
}

#[test]
fn a_crop_in_the_same_jump_shifts_the_pending_paths() {
    let mut order = UndoOrder::default();
    record(&mut order, "Pen: Add Point", PathSet::new(), set_with(10.0));
    assert!(order.apply_path_step(UndoDirection::Undo));
    // A crop step undone after the path step in the same jump: the pending
    // target is in the coordinates before the shift, so it moves too.
    order.undo.push(UndoKind::Crop);
    order.crop_undo.push(crate::crop::CropStep {
        dx: 4,
        dy: 6,
        before: (100, 100),
        after: (50, 50),
    });
    order.pending_paths = Some(set_with(10.0));
    assert!(order.apply_crop_step(UndoDirection::Undo));
    let pending = order.take_pending_paths();
    let first = pending
        .as_ref()
        .and_then(|p| p.get("A"))
        .and_then(|p| p.subpaths.first())
        .and_then(|s| s.anchors.first().copied());
    assert_eq!(first.map(|a| a.point), Some((6.0, -4.0)), "shifted by -dx, -dy");
}

#[test]
fn the_history_panel_names_a_path_step() {
    let mut order = UndoOrder::default();
    record(&mut order, "Close Path", PathSet::new(), set_with(1.0));
    let steps = order.history_steps();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].label, "Close Path");
    assert!(!steps[0].undone);
    assert_eq!(crate::default_step_label(UndoKind::Path), "Path Change");
}
