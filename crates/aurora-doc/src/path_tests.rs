//! Tests of the path model (0.233.0): bounds, names, flattening accuracy
//! against analytic arcs, the rasteriser against the Lasso's polygon filler,
//! open paths, several subpaths and holes, and the set's operations.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::excessive_precision,
    clippy::float_cmp,
    clippy::manual_midpoint,
    clippy::many_single_char_names
)]

use aurora_core::Rect;

use crate::{
    Anchor, DocError, FLATTEN_TOLERANCE, MAX_ANCHORS_PER_SUBPATH, MAX_PATH_ANCHORS_TOTAL,
    MAX_PATHS, MAX_SUBPATHS, Path, PathSet, SelectionSet, SelectionShape, Subpath, WORK_PATH_NAME,
    polygon_mask, selection_from_path, validate_path_name,
};

fn poly_path(name: &str, pts: &[(f32, f32)], closed: bool) -> Path {
    Path {
        name: name.to_owned(),
        subpaths: vec![Subpath {
            anchors: pts.iter().map(|&(x, y)| Anchor::corner(x, y)).collect(),
            closed,
        }],
    }
}

fn square(name: &str, x: f32, y: f32, side: f32, closed: bool) -> Path {
    poly_path(
        name,
        &[(x, y), (x + side, y), (x + side, y + side), (x, y + side)],
        closed,
    )
}

fn mask_of(path: &Path, size: (u32, u32)) -> Option<std::sync::Arc<crate::SelectionMask>> {
    match selection_from_path(path, size) {
        Ok(Some(selection)) => match selection.shape {
            SelectionShape::Mask(mask) => Some(mask),
            _ => None,
        },
        _ => None,
    }
}

fn cov8(m: &crate::SelectionMask, x: i64, y: i64) -> u8 {
    (m.coverage_at(x, y) * 255.0).round() as u8
}

fn coverage(path: &Path, size: (u32, u32), x: i64, y: i64) -> u8 {
    mask_of(path, size).map_or(0, |m| cov8(&m, x, y))
}

#[test]
fn names_are_trimmed_checked_and_the_work_name_is_reserved() {
    assert_eq!(
        validate_path_name("  Outline ").as_deref().ok(),
        Some("Outline")
    );
    assert!(validate_path_name("   ").is_err());
    assert!(validate_path_name("a\u{7}b").is_err());
    assert!(validate_path_name(&"x".repeat(65)).is_err());
    assert!(validate_path_name(&"x".repeat(64)).is_ok());
    assert!(validate_path_name("work path").is_err());
    assert!(validate_path_name(WORK_PATH_NAME).is_err());
}

#[test]
fn validation_enforces_coordinates_and_counts() {
    let mut p = square("a", 0.0, 0.0, 10.0, true);
    assert!(p.validate().is_ok());
    p.subpaths[0].anchors[0].point = (f32::NAN, 0.0);
    assert!(p.validate().is_err());
    p.subpaths[0].anchors[0] = Anchor::corner(1.0e7 + 1.0, 0.0);
    assert!(p.validate().is_err());
    p.subpaths[0].anchors[0] = Anchor::corner(1.0e7, -1.0e7);
    assert!(p.validate().is_ok(), "the limit itself is allowed");
    // A handle that lands out of range is refused too.
    p.subpaths[0].anchors[0] = Anchor::smooth((9.9e6, 0.0), (5.0e6, 0.0));
    assert!(p.validate().is_err());
    let many = Path {
        name: "m".into(),
        subpaths: vec![
            Subpath {
                anchors: vec![Anchor::corner(0.0, 0.0); MAX_ANCHORS_PER_SUBPATH],
                closed: true
            };
            1
        ],
    };
    assert!(many.validate().is_ok());
    let mut over = many.clone();
    over.subpaths[0].anchors.push(Anchor::corner(1.0, 1.0));
    assert!(over.validate().is_err());
    let subs = Path {
        name: "s".into(),
        subpaths: vec![Subpath::default(); MAX_SUBPATHS],
    };
    assert!(subs.validate().is_ok());
    let mut over = subs;
    over.subpaths.push(Subpath::default());
    assert!(over.validate().is_err());
}

#[test]
fn the_set_caps_paths_names_and_total_anchors() {
    let mut set = PathSet::new();
    for i in 0..MAX_PATHS {
        assert!(set.add(&format!("p{i}")).is_ok());
    }
    assert!(set.add("one more").is_err());
    assert!(set.ensure_work().is_err(), "the work path counts as well");
    let mut set = PathSet::new();
    assert!(set.add("a").is_ok());
    assert!(set.add("a").is_err(), "a name is unique");
    assert!(set.add("A").is_ok(), "names are case sensitive");
    // Total anchors.
    let big = |name: &str, n: usize| Path {
        name: name.into(),
        subpaths: vec![Subpath {
            anchors: vec![Anchor::corner(1.0, 1.0); n],
            closed: false,
        }],
    };
    let per = MAX_ANCHORS_PER_SUBPATH;
    let n = MAX_PATH_ANCHORS_TOTAL / per;
    let ok: Vec<Path> = (0..n).map(|i| big(&format!("p{i}"), per)).collect();
    assert!(PathSet::from_parts(ok.clone(), None).is_ok());
    let mut over = ok;
    over.push(big("extra", 1));
    assert!(PathSet::from_parts(over, None).is_err());
    // Bad parts.
    assert!(PathSet::from_parts(vec![big("x", 1), big("x", 1)], None).is_err());
    assert!(PathSet::from_parts(vec![big("", 1)], None).is_err());
    assert!(PathSet::from_parts(vec![], Some(big("not the work name", 1))).is_err());
    assert!(PathSet::from_parts(vec![big(WORK_PATH_NAME, 1)], None).is_err());
}

#[test]
fn rename_saves_the_work_path_and_delete_removes_it() {
    let mut set = PathSet::new();
    assert!(set.ensure_work().is_ok());
    assert_eq!(set.names().collect::<Vec<_>>(), vec![WORK_PATH_NAME]);
    assert_eq!(
        set.rename(WORK_PATH_NAME, " Saved ").ok().as_deref(),
        Some("Saved")
    );
    assert!(set.work().is_none());
    assert_eq!(set.names().collect::<Vec<_>>(), vec!["Saved"]);
    assert!(set.rename("Saved", "Saved").is_ok(), "same name is fine");
    assert!(set.add("Other").is_ok());
    assert!(set.rename("Saved", "Other").is_err());
    assert!(set.rename("Nope", "X").is_err());
    assert!(set.delete("Nope").is_err());
    assert!(set.delete("Saved").is_ok());
    assert_eq!(set.next_free_name(), "Path 1");
    assert!(set.add("Path 1").is_ok());
    assert_eq!(set.next_free_name(), "Path 2");
    assert!(set.ensure_work().is_ok());
    assert!(set.delete(WORK_PATH_NAME).is_ok());
    assert!(set.work().is_none());
}

#[test]
fn set_subpaths_is_all_or_nothing() {
    let mut set = PathSet::new();
    assert!(set.add("a").is_ok());
    let good = square("a", 0.0, 0.0, 4.0, true).subpaths;
    assert!(set.set_subpaths("a", good.clone()).is_ok());
    let mut bad = good;
    bad[0].anchors[0].point = (f32::INFINITY, 0.0);
    let before = set.clone();
    assert!(set.set_subpaths("a", bad).is_err());
    assert_eq!(set, before);
    assert!(set.set_subpaths("missing", vec![]).is_err());
}

#[test]
fn flattening_a_quarter_circle_stays_within_the_tolerance() {
    // The standard cubic approximation of a quarter circle of radius r.
    let r = 200.0_f32;
    let k = 0.552_284_75_f32 * r;
    let sub = Subpath {
        anchors: vec![
            Anchor {
                point: (r, 0.0),
                handle_in: (0.0, 0.0),
                handle_out: (0.0, k),
            },
            Anchor {
                point: (0.0, r),
                handle_in: (k, 0.0),
                handle_out: (0.0, 0.0),
            },
        ],
        closed: false,
    };
    let pts = sub.flatten(FLATTEN_TOLERANCE);
    assert!(pts.len() > 8, "a curve is subdivided");
    assert_eq!(pts.first().copied(), Some((r, 0.0)));
    assert_eq!(pts.last().copied(), Some((0.0, r)));
    // The cubic itself is within 2.7e-4 r of the circle; the flattening adds
    // at most the tolerance (vertices lie on the curve, so the chord's
    // midpoints are the farthest points, inside the circle).
    let slack = FLATTEN_TOLERANCE + 3.0e-4 * r;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let d = mid.0.hypot(mid.1);
        assert!(
            (d - r).abs() <= slack,
            "chord midpoint off the circle by {}",
            d - r
        );
        let va = a.0.hypot(a.1);
        assert!((va - r).abs() <= 3.0e-4 * r + 1.0e-3);
    }
}

#[test]
fn a_straight_segment_is_not_subdivided_and_a_corner_path_is_its_polygon() {
    let p = square("a", 0.0, 0.0, 10.0, true);
    let polys = p.flatten(FLATTEN_TOLERANCE);
    assert_eq!(polys.len(), 1);
    assert_eq!(polys[0].len(), 4, "closed: last point not repeated");
    let open = square("a", 0.0, 0.0, 10.0, false);
    assert_eq!(open.flatten(FLATTEN_TOLERANCE)[0].len(), 4);
    // Closing a smooth subpath adds the closing curve's points.
    let mut smooth = square("a", 0.0, 0.0, 100.0, true);
    for a in &mut smooth.subpaths[0].anchors {
        a.handle_out = (20.0, 0.0);
        a.handle_in = (-20.0, 0.0);
    }
    let closed = smooth.flatten(FLATTEN_TOLERANCE)[0].len();
    smooth.subpaths[0].closed = false;
    assert!(closed > smooth.flatten(FLATTEN_TOLERANCE)[0].len());
}

#[test]
fn a_polygon_path_selects_exactly_what_polygon_mask_gives() {
    let pts = [
        (3.0_f32, 2.5),
        (40.0, 6.0),
        (31.5, 33.0),
        (8.0, 28.0),
        (2.0, 15.0),
    ];
    let path = poly_path("p", &pts, true);
    let want = polygon_mask(
        &pts.iter()
            .map(|&(x, y)| (f64::from(x), f64::from(y)))
            .collect::<Vec<_>>(),
        &Rect {
            x: 0,
            y: 0,
            width: 50,
            height: 40,
        },
    );
    let want = want.ok().flatten();
    let got = mask_of(&path, (50, 40));
    assert!(want.is_some() && got.is_some());
    let (want, got) = (
        want.unwrap_or_else(|| unreachable!()),
        got.unwrap_or_else(|| unreachable!()),
    );
    assert_eq!(want.bounds(), got.bounds());
    assert_eq!(want.data(), got.data());
}

#[test]
fn an_open_path_is_closed_implicitly() {
    let open = square("o", 4.0, 4.0, 10.0, false);
    let closed = square("o", 4.0, 4.0, 10.0, true);
    let (a, b) = (mask_of(&open, (30, 30)), mask_of(&closed, (30, 30)));
    assert!(a.is_some());
    assert_eq!(a.map(|m| m.data().to_vec()), b.map(|m| m.data().to_vec()));
    assert_eq!(coverage(&open, (30, 30), 8, 8), 255);
    assert_eq!(coverage(&open, (30, 30), 20, 8), 0);
}

#[test]
fn several_subpaths_fill_together_with_nonzero_winding() {
    let mut p = square("two", 2.0, 2.0, 6.0, true);
    p.subpaths
        .extend(square("two", 20.0, 2.0, 6.0, true).subpaths);
    let size = (40, 20);
    assert_eq!(coverage(&p, size, 4, 4), 255);
    assert_eq!(coverage(&p, size, 22, 4), 255);
    assert_eq!(coverage(&p, size, 14, 4), 0, "nothing between them");
    // A same-direction inner square is filled once (still inside) ...
    let mut nested = square("n", 2.0, 2.0, 30.0, true);
    nested
        .subpaths
        .extend(square("n", 10.0, 10.0, 10.0, true).subpaths);
    assert_eq!(coverage(&nested, (40, 40), 14, 14), 255);
    // ... and an opposite-direction one is a hole.
    let mut hole = square("h", 2.0, 2.0, 30.0, true);
    let mut inner = square("h", 10.0, 10.0, 10.0, true).subpaths;
    inner[0].anchors.reverse();
    hole.subpaths.extend(inner);
    assert_eq!(coverage(&hole, (40, 40), 14, 14), 0);
    assert_eq!(coverage(&hole, (40, 40), 5, 5), 255);
    assert_eq!(coverage(&hole, (40, 40), 25, 25), 255);
    // The hub detour leaves no sliver along the connecting line.
    assert_eq!(coverage(&hole, (40, 40), 2, 14), 255);
}

#[test]
fn degenerate_and_empty_paths_select_nothing() {
    let size = (20, 20);
    assert!(
        selection_from_path(&Path::new("e"), size)
            .ok()
            .flatten()
            .is_none()
    );
    let two = poly_path("t", &[(1.0, 1.0), (9.0, 9.0)], true);
    assert!(selection_from_path(&two, size).ok().flatten().is_none());
    let line = poly_path("l", &[(1.0, 1.0), (9.0, 1.0), (5.0, 1.0)], true);
    assert!(selection_from_path(&line, size).ok().flatten().is_none());
    // Wholly outside the document: nothing either.
    let out = square("o", 100.0, 100.0, 5.0, true);
    assert!(selection_from_path(&out, size).ok().flatten().is_none());
    // Partly outside is clipped to the document.
    let half = square("h", -5.0, 2.0, 10.0, true);
    assert_eq!(coverage(&half, size, 0, 3), 255);
    assert_eq!(mask_of(&half, size).map(|m| m.bounds().x), Some(0));
    // An invalid path is an error, not a selection.
    let mut bad = square("b", 0.0, 0.0, 5.0, true);
    bad.subpaths[0].anchors[1].point.0 = f32::NAN;
    assert!(matches!(
        selection_from_path(&bad, size),
        Err(DocError::InvalidPath(_))
    ));
}

#[test]
fn a_curved_path_selects_a_disc_of_about_the_right_area() {
    let r = 30.0_f32;
    let k = 0.552_284_75_f32 * r;
    let c = 40.0_f32;
    let at = |dx: f32, dy: f32, ox: f32, oy: f32| {
        let mut a = Anchor::corner(c + dx, c + dy);
        a.handle_out = (ox, oy);
        a.handle_in = (-ox, -oy);
        a
    };
    let disc = Path {
        name: "disc".into(),
        subpaths: vec![Subpath {
            anchors: vec![
                at(r, 0.0, 0.0, k),
                at(0.0, r, -k, 0.0),
                at(-r, 0.0, 0.0, -k),
                at(0.0, -r, k, 0.0),
            ],
            closed: true,
        }],
    };
    let mask = mask_of(&disc, (80, 80));
    assert!(mask.is_some(), "a disc selects something");
    let Some(mask) = mask else {
        return;
    };
    let sum: f64 = mask.data().iter().map(|&v| f64::from(v) / 255.0).sum();
    let want = std::f64::consts::PI * f64::from(r) * f64::from(r);
    assert!((sum - want).abs() / want < 0.01, "area {sum} vs {want}");
    assert_eq!(cov8(&mask, 40, 40), 255);
    assert_eq!(cov8(&mask, 2, 2), 0);
}

#[test]
fn the_selection_set_carries_paths_and_the_crop_shifts_them() {
    let mut set = SelectionSet::new();
    let mut paths = PathSet::new();
    assert!(paths.add("a").is_ok());
    assert!(
        paths
            .set_subpaths("a", square("a", 10.0, 20.0, 5.0, true).subpaths)
            .is_ok()
    );
    set.set_paths(paths);
    assert!(set.is_empty(), "paths are not selections");
    assert!(set.translate(-4, -8).is_ok());
    let first = set
        .paths()
        .get("a")
        .and_then(|p| p.subpaths[0].anchors.first().copied());
    assert_eq!(first.map(|a| a.point), Some((6.0, 12.0)));
    assert!(set.translate(4, 8).is_ok());
    let first = set
        .paths()
        .get("a")
        .and_then(|p| p.subpaths[0].anchors.first().copied());
    assert_eq!(first.map(|a| a.point), Some((10.0, 20.0)));
    // A shift out of range changes nothing.
    assert!(set.translate(20_000_000, 0).is_err());
    let first = set
        .paths()
        .get("a")
        .and_then(|p| p.subpaths[0].anchors.first().copied());
    assert_eq!(first.map(|a| a.point), Some((10.0, 20.0)));
}

#[test]
fn live_edits_through_subpaths_mut_are_caught_by_validate() {
    let mut set = PathSet::new();
    assert!(set.add("a").is_ok());
    assert!(set.ensure_work().is_ok());
    for name in ["a", WORK_PATH_NAME] {
        assert!(set.validate().is_ok());
        let Some(subs) = set.subpaths_mut(name) else {
            unreachable!("exists")
        };
        subs.push(Subpath {
            anchors: vec![Anchor::corner(f32::NAN, 0.0)],
            closed: false,
        });
        assert!(set.validate().is_err(), "{name}");
        let Some(subs) = set.subpaths_mut(name) else {
            unreachable!("exists")
        };
        subs.clear();
    }
    assert!(set.subpaths_mut("missing").is_none());
}

#[test]
fn three_subpaths_leave_nothing_between_them() {
    // The chaining detour must return to the hub after every subpath, or a
    // stray edge joins the second and third squares.
    let mut p = square("three", 2.0, 2.0, 6.0, true);
    p.subpaths
        .extend(square("three", 20.0, 2.0, 6.0, true).subpaths);
    p.subpaths
        .extend(square("three", 11.0, 20.0, 6.0, true).subpaths);
    let size = (40, 40);
    let Some(mask) = mask_of(&p, size) else {
        unreachable!("selects");
    };
    let total: f64 = mask.data().iter().map(|&v| f64::from(v) / 255.0).sum();
    assert!((total - 108.0).abs() < 1.0e-6, "area {total}");
    for (x, y) in [(5, 5), (22, 5), (13, 22)] {
        assert_eq!(cov8(&mask, x, y), 255);
    }
    for (x, y) in [(14, 5), (14, 12), (5, 14), (25, 14), (14, 30)] {
        assert_eq!(cov8(&mask, x, y), 0, "({x}, {y})");
    }
}
