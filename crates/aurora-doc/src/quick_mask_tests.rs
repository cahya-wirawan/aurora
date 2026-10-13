//! Tests of [`QuickMask`] (0.217.0): selection -> grid -> selection round
//! trips for every shape (inverted included), the three outcomes, the
//! tight crop, the edit counter and the cap.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::sync::Arc;

use aurora_core::Rect;

use crate::{
    DocError, MAX_SELECTION_MASK_PIXELS, QuickMask, QuickMaskOutcome, Selection, SelectionMask,
    SelectionShape,
};

const DOC: (u32, u32) = (40, 30);

fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn byte_of(selection: &Selection, x: i64, y: i64) -> u8 {
    (selection.coverage_at(x, y) * 255.0).round() as u8
}

/// Every in-document texel of `a` and `b` agrees within `tolerance` bytes.
fn same_coverage(a: &Selection, b: &Selection, tolerance: u8) -> bool {
    (0..i64::from(DOC.1)).all(|y| {
        (0..i64::from(DOC.0)).all(|x| byte_of(a, x, y).abs_diff(byte_of(b, x, y)) <= tolerance)
    })
}

fn region(outcome: QuickMaskOutcome) -> Selection {
    match outcome {
        QuickMaskOutcome::Region(selection) => selection,
        other => unreachable!("expected a region, got {other:?}"),
    }
}

fn soft_mask() -> Selection {
    let bounds = rect(5, 4, 12, 9);
    let data: Vec<u8> = (0..108_u32).map(|i| ((i * 37) % 256) as u8).collect();
    // Make sure it has a full and an empty texel.
    let mut data = data;
    data[0] = 255;
    data[1] = 0;
    Selection::from_mask(Arc::new(
        SelectionMask::new(bounds, data).unwrap_or_else(|e| unreachable!("fixture: {e:?}")),
    ))
}

#[test]
fn no_selection_is_a_full_grid_and_comes_back_as_everything() {
    let q = QuickMask::from_selection(None, DOC).unwrap_or_else(|e| unreachable!("{e:?}"));
    assert!((0..30).all(|y| {
        q.row(y)
            .is_some_and(|r| r.len() == 40 && r.iter().all(|&v| v == 255))
    }));
    assert_eq!(q.outcome(), QuickMaskOutcome::Everything);
    assert!(!q.is_edited());
}

#[test]
fn a_rectangle_round_trips_exactly_as_a_rectangle() {
    let sel = Selection::new(rect(3, 2, 20, 10));
    let q = QuickMask::from_selection(Some(&sel), DOC).unwrap_or_else(|e| unreachable!("{e:?}"));
    assert_eq!(q.byte_at(3, 2), 255);
    assert_eq!(q.byte_at(22, 11), 255);
    assert_eq!(q.byte_at(23, 11), 0);
    assert_eq!(q.byte_at(2, 2), 0);
    let back = region(q.outcome());
    assert_eq!(back, sel, "a rectangle comes back as the same rectangle");
}

#[test]
fn every_shape_and_inversion_round_trips_within_one_byte() {
    let mut shapes = vec![
        Selection::new(rect(3, 2, 20, 10)),
        Selection::ellipse(rect(4, 3, 25, 18)),
        soft_mask(),
    ];
    let inverted: Vec<Selection> = shapes
        .iter()
        .map(|s| Selection {
            inverted: true,
            ..s.clone()
        })
        .collect();
    shapes.extend(inverted);
    for sel in &shapes {
        let q = QuickMask::from_selection(Some(sel), DOC).unwrap_or_else(|e| unreachable!("{e:?}"));
        for y in 0..i64::from(DOC.1) {
            for x in 0..i64::from(DOC.0) {
                assert!(
                    q.byte_at(x, y).abs_diff(byte_of(sel, x, y)) <= 1,
                    "{:?} at {x},{y}",
                    sel.shape
                );
            }
        }
        let back = region(q.outcome());
        assert!(
            same_coverage(sel, &back, 1),
            "{:?} inverted={}",
            sel.shape,
            sel.inverted
        );
        assert!(!back.inverted, "the result is never an inverted selection");
    }
}

#[test]
fn an_ellipse_in_the_grid_matches_its_own_coverage_bytes() {
    let sel = Selection::ellipse(rect(4, 3, 25, 18));
    let q = QuickMask::from_selection(Some(&sel), DOC).unwrap_or_else(|e| unreachable!("{e:?}"));
    for y in 0..30 {
        for x in 0..40 {
            assert!(
                q.byte_at(x, y).abs_diff(byte_of(&sel, x, y)) <= 1,
                "{x},{y}"
            );
        }
    }
}

#[test]
fn a_selection_beyond_the_document_is_clipped_to_it() {
    let sel = Selection::new(rect(-10, -10, 30, 30));
    let q = QuickMask::from_selection(Some(&sel), DOC).unwrap_or_else(|e| unreachable!("{e:?}"));
    let back = region(q.outcome());
    assert_eq!(back.bounds, rect(0, 0, 20, 20));
}

#[test]
fn all_zero_is_nothing_and_all_full_is_everything() {
    let mut q = QuickMask::from_selection(None, DOC).unwrap_or_else(|e| unreachable!("{e:?}"));
    q.edit(|data, _, _| {
        data.fill(0);
        Some(rect(0, 0, DOC.0, DOC.1))
    });
    assert_eq!(q.outcome(), QuickMaskOutcome::Nothing);
    q.edit(|data, _, _| {
        data.fill(255);
        Some(rect(0, 0, DOC.0, DOC.1))
    });
    assert_eq!(q.outcome(), QuickMaskOutcome::Everything);
}

#[test]
fn the_result_is_cropped_to_the_non_zero_texels_and_a_full_block_is_a_rectangle() {
    let mut q = QuickMask::from_selection(None, DOC).unwrap_or_else(|e| unreachable!("{e:?}"));
    q.edit(|data, w, _| {
        data.fill(0);
        for y in 5..9usize {
            for x in 7..13usize {
                data[y * w as usize + x] = 255;
            }
        }
        Some(rect(0, 0, DOC.0, DOC.1))
    });
    let sel = region(q.outcome());
    assert_eq!(sel, Selection::new(rect(7, 5, 6, 4)));
    // One grey texel makes it a mask with the same tight bounds.
    q.edit(|data, w, _| {
        data[5 * w as usize + 7] = 100;
        Some(rect(7, 5, 1, 1))
    });
    let sel = region(q.outcome());
    assert_eq!(sel.bounds, rect(7, 5, 6, 4));
    assert!(matches!(sel.shape, SelectionShape::Mask(_)));
    assert_eq!(byte_of(&sel, 7, 5), 100);
    assert_eq!(byte_of(&sel, 8, 5), 255);
}

#[test]
fn edits_count_only_when_something_changed() {
    let mut q = QuickMask::from_selection(None, DOC).unwrap_or_else(|e| unreachable!("{e:?}"));
    assert_eq!(q.edit(|_, _, _| None), None);
    assert_eq!(q.version(), 0);
    assert!(q.edit(|_, _, _| Some(rect(0, 0, 1, 1))).is_some());
    assert_eq!(q.version(), 1);
    assert!(q.is_edited());
}

#[test]
fn the_cap_and_the_empty_document_are_refused() {
    let side = 8193;
    assert!(u64::from(side) * u64::from(side) > MAX_SELECTION_MASK_PIXELS);
    assert!(matches!(
        QuickMask::from_selection(None, (side, side)),
        Err(DocError::SelectionMaskTooLarge(n)) if n == u64::from(side) * u64::from(side)
    ));
    assert!(matches!(
        QuickMask::from_selection(None, (0, 10)),
        Err(DocError::SelectionMaskInvalid)
    ));
}

#[test]
fn byte_at_and_row_are_zero_or_none_outside_the_grid() {
    let q = QuickMask::from_selection(None, DOC).unwrap_or_else(|e| unreachable!("{e:?}"));
    assert_eq!(q.byte_at(-1, 0), 0);
    assert_eq!(q.byte_at(40, 0), 0);
    assert_eq!(q.byte_at(0, 30), 0);
    assert!(q.row(30).is_none());
    assert!(q.row(-1).is_none());
}
