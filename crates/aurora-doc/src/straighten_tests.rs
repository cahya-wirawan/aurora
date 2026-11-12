//! Tests of the Straighten's document half (task 180): `LayerTree::set_all_bounds`,
//! `History::set_all_bounds`, the `SetAllBounds` journal op and
//! `PathSet::transform`.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::expect_used,
    clippy::float_cmp
)]

use aurora_core::{MAX_DOCUMENT_ORIGIN, Rect};

use crate::{Anchor, DocError, History, LayerId, LayerTree, Path, PathSet, Subpath};

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
    a: LayerId,
    b: LayerId,
    group: LayerId,
}

fn rig() -> Rig {
    let mut tree = LayerTree::new();
    let mut history = History::new();
    let a = history
        .add_pixel_layer(&mut tree, "a", rect(-10, 4, 50, 30), None)
        .expect("a");
    let group = history.add_group(&mut tree, "g", None).expect("group");
    let b = history
        .add_pixel_layer(&mut tree, "b", rect(7, 9, 20, 10), Some(group))
        .expect("b");
    tree.add_mask(a, rect(1, 2, 40, 30)).expect("mask a");
    tree.add_mask(group, rect(2, 3, 12, 11)).expect("mask g");
    Rig {
        tree,
        history,
        a,
        b,
        group,
    }
}

#[test]
fn set_all_bounds_sets_layers_and_masks_and_undo_redo_restore_them_exactly() {
    let mut r = rig();
    let before = r.tree.all_bounds();
    let new = rect(0, 0, 33, 22);
    r.history
        .set_all_bounds(
            &mut r.tree,
            vec![(r.a, new), (r.b, new)],
            vec![(r.a, new), (r.group, new)],
        )
        .expect("set");
    assert_eq!(r.tree.bounds(r.a), Some(new));
    assert_eq!(r.tree.bounds(r.b), Some(new));
    assert_eq!(r.tree.mask(r.a).map(|m| m.bounds), Some(new));
    assert_eq!(r.tree.mask(r.group).map(|m| m.bounds), Some(new));
    let after = r.tree.all_bounds();
    assert!(r.history.undo(&mut r.tree).is_ok());
    assert_eq!(r.tree.all_bounds(), before);
    assert!(r.history.redo(&mut r.tree).is_ok());
    assert_eq!(r.tree.all_bounds(), after);
}

#[test]
fn it_is_one_undo_step() {
    let mut r = rig();
    let new = rect(0, 0, 9, 9);
    let first = r.tree.all_bounds();
    r.history
        .set_all_bounds(&mut r.tree, vec![(r.a, new), (r.b, new)], Vec::new())
        .expect("set");
    // One undo undoes the whole retargeting.
    assert!(r.history.undo(&mut r.tree).is_ok());
    assert_eq!(r.tree.all_bounds(), first);
}

#[test]
fn a_bad_list_changes_and_records_nothing() {
    let mut r = rig();
    let before = r.tree.all_bounds();
    let journal = r.history.save_journal().expect("journal");
    let new = rect(0, 0, 9, 9);
    // An origin out of range, a group in the pixel list, an unknown id and a
    // mask that does not exist: each refuses the whole call.
    let far = rect(MAX_DOCUMENT_ORIGIN + 1, 0, 1, 1);
    assert!(matches!(
        r.history
            .set_all_bounds(&mut r.tree, vec![(r.a, new), (r.b, far)], Vec::new()),
        Err(DocError::LayerOriginOutOfRange { .. })
    ));
    assert!(matches!(
        r.history
            .set_all_bounds(&mut r.tree, vec![(r.a, new), (r.group, new)], Vec::new()),
        Err(DocError::NotAPixelLayer(_))
    ));
    assert!(matches!(
        r.history.set_all_bounds(
            &mut r.tree,
            vec![(r.a, new), (LayerId::from_raw(9999), new)],
            Vec::new()
        ),
        Err(DocError::UnknownLayer(_))
    ));
    assert!(matches!(
        r.history
            .set_all_bounds(&mut r.tree, vec![(r.a, new)], vec![(r.b, new)]),
        Err(DocError::UnknownLayer(_))
    ));
    assert_eq!(r.tree.all_bounds(), before);
    assert_eq!(r.history.save_journal().expect("journal"), journal);
}

#[test]
fn the_journal_replays_to_the_same_bounds() {
    let dir = tempfile::tempdir().expect("dir");
    let budget = std::num::NonZeroUsize::new(4).expect("non-zero");
    let mut store = aurora_tile::TileStore::new(dir.path().to_path_buf(), budget).expect("store");
    let mut tree = LayerTree::new();
    let mut history = History::new();
    let a = history
        .add_pixel_layer(&mut tree, "a", rect(-10, 4, 50, 30), None)
        .expect("a");
    let group = history.add_group(&mut tree, "g", None).expect("group");
    let b = history
        .add_pixel_layer(&mut tree, "b", rect(7, 9, 20, 10), Some(group))
        .expect("b");
    history
        .add_mask(&mut tree, &mut store, a, rect(1, 2, 40, 30))
        .expect("mask");
    history
        .add_mask(&mut tree, &mut store, group, rect(2, 3, 12, 11))
        .expect("group mask");
    let new = rect(0, 0, 41, 17);
    history
        .set_all_bounds(
            &mut tree,
            vec![(a, new), (b, new)],
            vec![(a, new), (group, new)],
        )
        .expect("set");
    let wire = history.save_journal().expect("journal");
    let replayed = History::load_journal(&wire)
        .expect("load")
        .replay()
        .expect("replay");
    assert_eq!(replayed.all_bounds(), tree.all_bounds());
    assert_eq!(replayed.bounds(b), Some(new));
}

#[test]
fn all_bounds_is_sorted_by_id_for_a_deterministic_journal() {
    let r = rig();
    let (pixel, masks) = r.tree.all_bounds();
    assert!(pixel.windows(2).all(|w| w[0].0.to_raw() < w[1].0.to_raw()));
    assert!(masks.windows(2).all(|w| w[0].0.to_raw() < w[1].0.to_raw()));
    assert_eq!(pixel.len(), 2);
    assert_eq!(masks.len(), 2);
}

fn sample_paths() -> PathSet {
    let mut path = Path::new("p");
    path.subpaths.push(Subpath {
        anchors: vec![
            Anchor {
                point: (10.0, 0.0),
                handle_in: (0.0, 0.0),
                handle_out: (4.0, 0.0),
            },
            Anchor::corner(0.0, 10.0),
        ],
        closed: false,
    });
    PathSet::from_parts(vec![path], None).expect("set")
}

#[test]
fn path_transform_maps_points_fully_and_handles_by_the_linear_part_only() {
    let mut set = sample_paths();
    // A quarter turn plus a shift: (x, y) -> (-y + 5, x + 7).
    set.transform([0.0, 1.0, -1.0, 0.0, 5.0, 7.0]).expect("ok");
    let anchors = &set.get("p").expect("p").subpaths[0].anchors;
    assert_eq!(anchors[0].point, (5.0, 17.0));
    assert_eq!(anchors[0].handle_out, (0.0, 4.0));
    assert_eq!(anchors[1].point, (-5.0, 7.0));
    assert_eq!(anchors[1].handle_in, (0.0, 0.0));
}

#[test]
fn path_transform_is_all_or_nothing_and_refuses_a_bad_matrix() {
    let mut set = sample_paths();
    let before = set.clone();
    assert!(set.transform([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]).is_err());
    assert!(set.transform([1.0e30, 0.0, 0.0, 1.0, 0.0, 0.0]).is_err());
    assert_eq!(set.get("p"), before.get("p"));
}
