//! Tests of [`combine_selections`] (0.215.0): a brute-force byte reference
//! over random rectangles, ellipses, soft masks and inverted operands,
//! identity and commutativity laws, bounds tightness, the rectangle
//! shortcuts and the mask cap.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::float_cmp
)]

use std::sync::Arc;

use aurora_core::Rect;

use crate::{
    DocError, Selection, SelectionMask, SelectionMode, SelectionShape, combine_selections,
};

fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// A small deterministic generator (no dependency).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn range(&mut self, lo: i64, n: u64) -> i64 {
        lo + self.below(n) as i64
    }
}

const WINDOW: (i64, i64) = (-6, 40);

fn random_rect(rng: &mut Lcg) -> Rect {
    rect(
        rng.range(0, 20),
        rng.range(0, 20),
        1 + rng.below(14) as u32,
        1 + rng.below(14) as u32,
    )
}

/// `binary`: only `0`/`255` bytes in masks and no ellipses.
fn random_selection(rng: &mut Lcg, binary: bool) -> Selection {
    let bounds = random_rect(rng);
    let mut selection = match rng.below(if binary { 2 } else { 3 }) {
        0 => Selection::new(bounds),
        1 if binary || rng.below(2) == 0 => {
            let pixels = bounds.area_px() as usize;
            let data: Vec<u8> = (0..pixels)
                .map(|_| {
                    let v = rng.below(5);
                    if binary {
                        if v < 3 { u8::MAX } else { 0 }
                    } else {
                        // Includes 0, 255 and awkward middles (127, 128).
                        [0, 255, 127, 128, 64, 255][v as usize % 6]
                    }
                })
                .collect();
            match SelectionMask::new(bounds, data) {
                Ok(mask) => Selection::from_mask(Arc::new(mask)),
                Err(err) => unreachable!("{err:?}"),
            }
        }
        _ => Selection::ellipse(bounds),
    };
    selection.inverted = rng.below(4) == 0;
    selection
}

/// The effective coverage byte of `s` at `(x, y)`, from the public
/// coverage API of the *un-inverted* shape (so it shares no code with the
/// combination).
fn eff(s: &Selection, x: i64, y: i64) -> u8 {
    let mut plain = s.clone();
    plain.inverted = false;
    let byte = (plain.coverage_at(x, y) * 255.0).round() as u8;
    if s.inverted { 255 - byte } else { byte }
}

fn eff_opt(s: Option<&Selection>, x: i64, y: i64) -> u8 {
    s.map_or(0, |s| eff(s, x, y))
}

fn reference(mode: SelectionMode, a: u8, b: u8) -> u8 {
    match mode {
        SelectionMode::Replace => b,
        SelectionMode::Add => a.max(b),
        SelectionMode::Subtract => a.saturating_sub(b),
        SelectionMode::Intersect => a.min(b),
    }
}

fn combine(a: Option<&Selection>, b: &Selection, mode: SelectionMode) -> Option<Selection> {
    match combine_selections(a, b, mode) {
        Ok(result) => result,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn assert_matches_reference(a: &Selection, b: &Selection, mode: SelectionMode, what: &str) {
    let result = combine(Some(a), b, mode);
    for y in WINDOW.0..WINDOW.1 {
        for x in WINDOW.0..WINDOW.1 {
            let want = reference(mode, eff(a, x, y), eff(b, x, y));
            assert_eq!(
                eff_opt(result.as_ref(), x, y),
                want,
                "{what} {mode:?} at ({x}, {y}): {a:?} / {b:?}"
            );
        }
    }
    // Far outside the window every operand is at its constant outside
    // value, and so is the result.
    for (x, y) in [(1000, 1000), (-1000, 5), (5, -1000)] {
        let want = reference(mode, eff(a, x, y), eff(b, x, y));
        assert_eq!(
            eff_opt(result.as_ref(), x, y),
            want,
            "{what} far ({x}, {y})"
        );
    }
}

#[test]
fn every_mode_matches_a_brute_force_byte_reference_on_random_operands() {
    let mut rng = Lcg(7);
    let mut compared = 0;
    for _ in 0..160 {
        let a = random_selection(&mut rng, false);
        let b = random_selection(&mut rng, false);
        for mode in SelectionMode::ALL {
            assert_matches_reference(&a, &b, mode, "random");
            compared += 1;
        }
    }
    assert_eq!(compared, 640);
}

#[test]
fn inverted_operands_are_exact_in_every_combination() {
    let a = Selection::new(rect(2, 2, 8, 8));
    let b = Selection::ellipse(rect(5, 4, 9, 9));
    for (ia, ib) in [(false, false), (true, false), (false, true), (true, true)] {
        let (mut a, mut b) = (a.clone(), b.clone());
        a.inverted = ia;
        b.inverted = ib;
        for mode in SelectionMode::ALL {
            assert_matches_reference(&a, &b, mode, &format!("inv {ia} {ib}"));
        }
    }
}

#[test]
fn results_are_never_an_empty_non_inverted_selection_and_bounds_are_tight() {
    let mut rng = Lcg(99);
    let mut nones = 0;
    let mut inverted_results = 0;
    for _ in 0..400 {
        let a = random_selection(&mut rng, false);
        let b = random_selection(&mut rng, false);
        for mode in SelectionMode::ALL {
            let Some(result) = combine(Some(&a), &b, mode) else {
                nones += 1;
                continue;
            };
            if result.inverted {
                inverted_results += 1;
            }
            // An operand returned as it came (a disjoint subtract) keeps
            // its own, possibly loose, bounds.
            if result == a || result == b {
                continue;
            }
            if result.bounds.is_empty() {
                // Only "everything": the inverted empty rectangle.
                assert!(result.inverted && result.is_rect(), "{result:?}");
                continue;
            }
            // The stored (background = 0) value touches all four edges of
            // the bounds, so the bounds are tight.
            let stored = |x: i64, y: i64| {
                let e = eff(&result, x, y);
                if result.inverted { 255 - e } else { e }
            };
            let b = result.bounds;
            let touches = |f: &dyn Fn(i64, i64) -> bool| {
                (b.y..b.bottom()).any(|y| (b.x..b.right()).any(|x| f(x, y)))
            };
            assert!(
                touches(&|x, y| stored(x, y) != 0 && y == b.y),
                "top {result:?}"
            );
            assert!(
                touches(&|x, y| stored(x, y) != 0 && y == b.bottom() - 1),
                "bottom {result:?}"
            );
            assert!(
                touches(&|x, y| stored(x, y) != 0 && x == b.x),
                "left {result:?}"
            );
            assert!(
                touches(&|x, y| stored(x, y) != 0 && x == b.right() - 1),
                "right {result:?}"
            );
        }
    }
    assert!(nones > 20, "{nones} empty results");
    assert!(inverted_results > 20, "{inverted_results} inverted results");
}

#[test]
fn add_and_intersect_are_commutative_and_idempotent() {
    let mut rng = Lcg(5);
    for _ in 0..120 {
        let a = random_selection(&mut rng, false);
        let b = random_selection(&mut rng, false);
        for mode in [SelectionMode::Add, SelectionMode::Intersect] {
            let ab = combine(Some(&a), &b, mode);
            let ba = combine(Some(&b), &a, mode);
            for y in WINDOW.0..WINDOW.1 {
                for x in WINDOW.0..WINDOW.1 {
                    assert_eq!(
                        eff_opt(ab.as_ref(), x, y),
                        eff_opt(ba.as_ref(), x, y),
                        "{mode:?} ({x}, {y})"
                    );
                }
            }
            let aa = combine(Some(&a), &a, mode);
            for y in WINDOW.0..WINDOW.1 {
                for x in WINDOW.0..WINDOW.1 {
                    assert_eq!(
                        eff_opt(aa.as_ref(), x, y),
                        eff(&a, x, y),
                        "{mode:?} self ({x}, {y})"
                    );
                }
            }
        }
    }
}

#[test]
fn subtracting_a_selection_from_itself_leaves_nothing() {
    let mut rng = Lcg(21);
    for _ in 0..80 {
        let a = random_selection(&mut rng, false);
        assert_eq!(
            combine(Some(&a), &a, SelectionMode::Subtract),
            None,
            "{a:?}"
        );
    }
}

#[test]
fn identities_with_nothing_and_with_disjoint_shapes() {
    let a = Selection::ellipse(rect(3, 3, 9, 7));
    let far = Selection::new(rect(500, 500, 4, 4));
    // Adding a disjoint rectangle keeps the old coverage and gains the new.
    let both = combine(Some(&a), &far, SelectionMode::Add);
    assert_eq!(eff_opt(both.as_ref(), 7, 6), eff(&a, 7, 6));
    assert_eq!(eff_opt(both.as_ref(), 501, 501), 255);
    // Subtracting a disjoint one changes nothing (the same selection back).
    assert_eq!(
        combine(Some(&a), &far, SelectionMode::Subtract),
        Some(a.clone())
    );
    // Intersecting with a disjoint one leaves nothing.
    assert_eq!(combine(Some(&a), &far, SelectionMode::Intersect), None);
    // Replace is the new one, whatever was there.
    assert_eq!(
        combine(Some(&a), &far, SelectionMode::Replace),
        Some(far.clone())
    );
    // An empty non-inverted new selection: Add changes nothing.
    let nothing = Selection::new(rect(4, 4, 0, 3));
    assert_eq!(
        combine(Some(&a), &nothing, SelectionMode::Add),
        Some(a.clone())
    );
    assert_eq!(combine(Some(&a), &nothing, SelectionMode::Replace), None);
}

#[test]
fn with_no_existing_selection_replace_and_add_take_the_new_one_and_the_others_nothing() {
    let n = Selection::ellipse(rect(1, 1, 6, 6));
    assert_eq!(combine(None, &n, SelectionMode::Replace), Some(n.clone()));
    assert_eq!(combine(None, &n, SelectionMode::Add), Some(n.clone()));
    assert_eq!(combine(None, &n, SelectionMode::Subtract), None);
    assert_eq!(combine(None, &n, SelectionMode::Intersect), None);
    // An empty new selection is nothing even for Add.
    let empty = Selection::new(rect(1, 1, 0, 6));
    assert_eq!(combine(None, &empty, SelectionMode::Add), None);
}

#[test]
fn for_hard_edges_subtract_is_intersect_with_the_complement() {
    let mut rng = Lcg(1234);
    for _ in 0..120 {
        let a = random_selection(&mut rng, true);
        let b = random_selection(&mut rng, true);
        let mut not_b = b.clone();
        not_b.inverted = !not_b.inverted;
        let sub = combine(Some(&a), &b, SelectionMode::Subtract);
        let via = combine(Some(&a), &not_b, SelectionMode::Intersect);
        for y in WINDOW.0..WINDOW.1 {
            for x in WINDOW.0..WINDOW.1 {
                assert_eq!(
                    eff_opt(sub.as_ref(), x, y),
                    eff_opt(via.as_ref(), x, y),
                    "({x}, {y})"
                );
            }
        }
    }
}

#[test]
fn rectangles_stay_rectangles_when_the_result_is_one() {
    let a = Selection::new(rect(0, 0, 10, 10));
    let b = Selection::new(rect(5, 5, 10, 10));
    let inter = combine(Some(&a), &b, SelectionMode::Intersect);
    assert_eq!(inter, Some(Selection::new(rect(5, 5, 5, 5))));
    // Contained rectangles: Add gives the outer one.
    let inner = Selection::new(rect(2, 2, 3, 3));
    assert_eq!(
        combine(Some(&a), &inner, SelectionMode::Add),
        Some(a.clone())
    );
    assert_eq!(
        combine(Some(&inner), &a, SelectionMode::Add),
        Some(a.clone())
    );
    // Adjacent rectangles that fill a rectangle are a rectangle too.
    let left = Selection::new(rect(0, 0, 5, 4));
    let right = Selection::new(rect(5, 0, 5, 4));
    let joined = combine(Some(&left), &right, SelectionMode::Add);
    assert_eq!(joined, Some(Selection::new(rect(0, 0, 10, 4))));
    // An L-shaped union needs a mask.
    let l = combine(Some(&a), &b, SelectionMode::Add);
    assert!(matches!(
        l.as_ref().map(|s| &s.shape),
        Some(SelectionShape::Mask(_))
    ));
    // Cutting a rectangle out of its middle needs a mask; cutting a strip
    // off its side leaves a rectangle.
    let strip = Selection::new(rect(0, 0, 10, 3));
    assert_eq!(
        combine(Some(&a), &strip, SelectionMode::Subtract),
        Some(Selection::new(rect(0, 3, 10, 7)))
    );
    let hole = combine(Some(&a), &inner, SelectionMode::Subtract);
    assert!(matches!(
        hole.as_ref().map(|s| &s.shape),
        Some(SelectionShape::Mask(_))
    ));
    // A rectangle covering the old one removes it.
    let all = Selection::new(rect(-1, -1, 30, 30));
    assert_eq!(combine(Some(&a), &all, SelectionMode::Subtract), None);
}

#[test]
fn huge_rectangles_combine_exactly_without_a_mask() {
    let a = Selection::new(rect(0, 0, 200_000, 200_000));
    let b = Selection::new(rect(100_000, 50_000, 200_000, 200_000));
    assert_eq!(
        combine(Some(&a), &b, SelectionMode::Intersect),
        Some(Selection::new(rect(100_000, 50_000, 100_000, 150_000)))
    );
    // Not a rectangle: refused by the cap, not a panic.
    assert!(matches!(
        combine_selections(Some(&a), &b, SelectionMode::Subtract),
        Err(DocError::SelectionMaskTooLarge(_))
    ));
}

#[test]
fn a_result_over_the_mask_cap_is_refused_before_allocating() {
    // Two small rectangles far apart: the union box is enormous.
    let a = Selection::new(rect(0, 0, 4, 4));
    let b = Selection::new(rect(20_000, 20_000, 4, 4));
    match combine_selections(Some(&a), &b, SelectionMode::Add) {
        Err(DocError::SelectionMaskTooLarge(n)) => assert!(n > 64 * 1024 * 1024, "{n}"),
        other => unreachable!("{other:?}"),
    }
    // Just under the cap works: 8192 x 8192 = 64 Mpx exactly.
    let ok_a = Selection::new(rect(0, 0, 1, 1));
    let ok_b = Selection::new(rect(8191, 8191, 1, 1));
    // Union box is 8192 x 8192 = exactly the cap.
    assert!(combine_selections(Some(&ok_a), &ok_b, SelectionMode::Add).is_ok());
    let over = Selection::new(rect(8192, 8191, 1, 1));
    assert!(matches!(
        combine_selections(Some(&ok_a), &over, SelectionMode::Add),
        Err(DocError::SelectionMaskTooLarge(_))
    ));
}

#[test]
fn an_inverted_operand_gives_an_inverted_result_over_the_non_inverted_bounds() {
    // Everything except a 10 x 10 block, plus a hole's worth of the block:
    // still inverted, bounded by the block.
    let mut outside = Selection::new(rect(0, 0, 10, 10));
    outside.inverted = true;
    let spot = Selection::new(rect(2, 2, 3, 3));
    let result = combine(Some(&outside), &spot, SelectionMode::Add);
    let Some(result) = result else {
        unreachable!("not nothing")
    };
    assert!(result.inverted);
    assert_eq!(result.bounds, rect(0, 0, 10, 10));
    assert_eq!(result.coverage_at(3, 3), 1.0, "the added spot");
    assert_eq!(result.coverage_at(8, 8), 0.0, "still unselected");
    assert_eq!(result.coverage_at(50, 50), 1.0, "outside stays selected");
    // Adding a rectangle that covers the unselected block selects all.
    let cover = Selection::new(rect(-2, -2, 20, 20));
    let all = combine(Some(&outside), &cover, SelectionMode::Add);
    let Some(all) = all else {
        unreachable!("everything is not nothing")
    };
    assert!(all.inverted && all.bounds.is_empty());
    assert_eq!(all.coverage_at(5, 5), 1.0);
    assert_eq!(all.coverage_at(-500, 9), 1.0);
    // Subtracting an inverted selection from itself is nothing, and
    // intersecting an inverted selection with a plain one clips the plain
    // one.
    assert_eq!(
        combine(Some(&outside), &outside, SelectionMode::Subtract),
        None
    );
    let clip = combine(
        Some(&outside),
        &Selection::new(rect(5, 5, 10, 10)),
        SelectionMode::Intersect,
    );
    let Some(clip) = clip else {
        unreachable!("an L-shaped remainder")
    };
    assert!(!clip.inverted);
    assert_eq!(clip.coverage_at(7, 7), 0.0, "inside the unselected block");
    assert_eq!(clip.coverage_at(12, 12), 1.0);
}
