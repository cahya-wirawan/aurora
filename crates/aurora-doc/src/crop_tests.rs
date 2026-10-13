//! Tests of the Crop's document half (0.226.0): `LayerTree::translate_all`,
//! `History::translate_all`, `Selection::translated`,
//! `SelectionSet::translate`.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::expect_used
)]

use std::sync::Arc;

use aurora_core::{MAX_DOCUMENT_ORIGIN, Rect};

use crate::{DocError, History, LayerTree, Selection, SelectionMask, SelectionSet};

fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

struct Rig {
    tree: LayerTree,
    history: History,
    pixel: crate::LayerId,
    inner: crate::LayerId,
    group: crate::LayerId,
}

fn rig() -> Rig {
    let mut tree = LayerTree::new();
    let mut history = History::new();
    let pixel = history
        .add_pixel_layer(&mut tree, "bottom", rect(-10, 4, 50, 30), None)
        .expect("layer");
    let group = history.add_group(&mut tree, "group", None).expect("group");
    let inner = history
        .add_pixel_layer(&mut tree, "inner", rect(7, 9, 20, 10), Some(group))
        .expect("inner");
    tree.add_mask(pixel, rect(0, 0, 40, 30)).expect("mask");
    tree.add_mask(group, rect(2, 3, 12, 11))
        .expect("group mask");
    Rig {
        tree,
        history,
        pixel,
        inner,
        group,
    }
}

#[test]
fn translate_all_shifts_every_pixel_layer_and_every_mask() {
    let mut r = rig();
    r.history
        .translate_all(&mut r.tree, -5, -3)
        .expect("translate");
    assert_eq!(r.tree.bounds(r.pixel), Some(rect(-15, 1, 50, 30)));
    assert_eq!(r.tree.bounds(r.inner), Some(rect(2, 6, 20, 10)));
    assert_eq!(
        r.tree.mask(r.pixel).map(|m| m.bounds),
        Some(rect(-5, -3, 40, 30))
    );
    assert_eq!(
        r.tree.mask(r.group).map(|m| m.bounds),
        Some(rect(-3, 0, 12, 11))
    );
    assert_eq!(r.tree.bounds(r.group), None, "a group has no bounds");
}

#[test]
fn undo_and_redo_restore_exactly_and_a_second_translate_after_undo_works() {
    let mut r = rig();
    let before_bounds = r.tree.bounds(r.pixel);
    let before_mask = r.tree.mask(r.group).map(|m| m.bounds);
    r.history.translate_all(&mut r.tree, -5, -3).expect("t");
    r.history.undo(&mut r.tree).expect("undo");
    assert_eq!(r.tree.bounds(r.pixel), before_bounds);
    assert_eq!(r.tree.mask(r.group).map(|m| m.bounds), before_mask);
    r.history.redo(&mut r.tree).expect("redo");
    assert_eq!(r.tree.bounds(r.pixel), Some(rect(-15, 1, 50, 30)));
    r.history.undo(&mut r.tree).expect("undo again");
    r.history.translate_all(&mut r.tree, 4, 8).expect("second");
    assert_eq!(r.tree.bounds(r.inner), Some(rect(11, 17, 20, 10)));
    assert!(!r.history.can_redo(), "a new step clears redo");
}

#[test]
fn an_out_of_range_shift_is_refused_and_changes_and_records_nothing() {
    let mut r = rig();
    let journal = r.history.journal_len();
    let err = r
        .history
        .translate_all(&mut r.tree, MAX_DOCUMENT_ORIGIN + 1, 0)
        .expect_err("refused");
    assert!(matches!(err, DocError::LayerOriginOutOfRange { .. }));
    assert_eq!(r.tree.bounds(r.pixel), Some(rect(-10, 4, 50, 30)));
    assert_eq!(
        r.tree.mask(r.group).map(|m| m.bounds),
        Some(rect(2, 3, 12, 11))
    );
    assert_eq!(r.history.journal_len(), journal);
    let overflow = r.history.translate_all(&mut r.tree, i64::MIN, 0);
    assert!(
        overflow.is_err(),
        "an overflowing shift is an error, not a wrap"
    );
}

#[test]
fn the_translate_step_survives_a_journal_round_trip() {
    let mut r = rig();
    r.history.translate_all(&mut r.tree, -1, -2).expect("t");
    let bytes = r.history.save_journal().expect("save");
    let loaded = History::load_journal(&bytes).expect("load");
    assert_eq!(loaded.journal_len(), r.history.journal_len());
    assert_eq!(
        loaded.journal_descriptions(),
        r.history.journal_descriptions()
    );
}

fn mask_selection() -> Selection {
    let mask =
        SelectionMask::rasterize(rect(4, 5, 6, 4), |x, y| ((x * 7 + y * 3) % 5) as f32 / 4.0)
            .expect("mask");
    Selection::from_mask(Arc::new(mask))
}

#[test]
fn a_translated_selection_has_the_same_coverage_at_the_shifted_point() {
    let mut inverted_ellipse = Selection::ellipse(rect(3, 3, 9, 7));
    inverted_ellipse.inverted = true;
    for selection in [
        Selection::new(rect(2, 2, 5, 5)),
        Selection::ellipse(rect(3, 3, 9, 7)),
        inverted_ellipse,
        mask_selection(),
    ] {
        let moved = selection.translated(-4, -2).expect("move");
        assert_eq!(moved.bounds.x, selection.bounds.x - 4);
        assert_eq!(moved.inverted, selection.inverted);
        for y in -2..20_i64 {
            for x in -4..24_i64 {
                assert!(
                    (moved.coverage_at(x - 4, y - 2) - selection.coverage_at(x, y)).abs() < 1e-6,
                    "({x}, {y})"
                );
            }
        }
        assert_eq!(moved.translated(4, 2).expect("back"), selection);
    }
}

#[test]
fn a_selection_set_translate_moves_the_active_and_every_saved_selection() {
    let mut set = SelectionSet::new();
    set.select(Selection::new(rect(10, 10, 4, 4)));
    set.save_active("a").expect("save");
    set.select(mask_selection());
    set.save_active("b").expect("save");
    set.translate(-3, -1).expect("translate");
    assert_eq!(set.active().map(|s| s.bounds.x), Some(1));
    assert_eq!(set.saved("a").map(|s| s.bounds), Some(rect(7, 9, 4, 4)));
    assert_eq!(set.saved("b").map(|s| s.bounds.y), Some(4));
    set.translate(3, 1).expect("back");
    assert_eq!(set.saved("a").map(|s| s.bounds), Some(rect(10, 10, 4, 4)));
}

#[test]
fn a_failed_selection_set_translate_changes_nothing() {
    let mut set = SelectionSet::new();
    set.select(Selection::new(rect(1, 1, 2, 2)));
    set.save_active("a").expect("save");
    set.select(Selection::new(rect(i64::MAX - 1, 1, 1, 1)));
    let before_saved = set.saved("a").cloned();
    assert!(set.translate(5, 0).is_err());
    assert_eq!(set.saved("a").cloned(), before_saved);
    assert_eq!(set.active().map(|s| s.bounds.x), Some(i64::MAX - 1));
}
