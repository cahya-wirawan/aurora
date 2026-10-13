//! Tests for the raster selection (0.213.0): the rectangle stays
//! bit-identical to 0.212.0, the ellipse and mask answer fractional
//! coverage, and the region queries are conservative.

#![allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::sync::Arc;

use aurora_core::Rect;

use crate::{
    DocError, MAX_SELECTION_MASK_PIXELS, MAX_TRACE_RUNS, Selection, SelectionMask, SelectionSet,
    SelectionShape,
};

fn r(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// A small deterministic generator (xorshift64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % u64::try_from(hi - lo).unwrap_or(1)) as i64
    }
    fn rect(&mut self, span: i64, max: i64) -> Rect {
        r(
            self.range(-span, span),
            self.range(-span, span),
            self.range(0, max) as u32,
            self.range(0, max) as u32,
        )
    }
}

/// The 0.212.0 formulas, verbatim, as the oracle for the rectangle.
mod old {
    use aurora_core::Rect;

    pub fn coverage_at(bounds: &Rect, inverted: bool, x: i64, y: i64) -> f32 {
        if bounds.contains_point(x, y) ^ inverted {
            1.0
        } else {
            0.0
        }
    }

    pub fn coverage_bounds(bounds: &Rect, inverted: bool) -> Option<Rect> {
        (!inverted).then_some(*bounds)
    }

    pub fn fully_covers(bounds: &Rect, inverted: bool, rect: &Rect) -> bool {
        if rect.is_empty() {
            return true;
        }
        if inverted {
            !bounds.intersects(rect)
        } else {
            bounds.x <= rect.x
                && bounds.y <= rect.y
                && bounds.right() >= rect.right()
                && bounds.bottom() >= rect.bottom()
        }
    }

    pub fn fully_excludes(bounds: &Rect, inverted: bool, rect: &Rect) -> bool {
        if rect.is_empty() {
            return true;
        }
        if inverted {
            bounds.x <= rect.x
                && bounds.y <= rect.y
                && bounds.right() >= rect.right()
                && bounds.bottom() >= rect.bottom()
        } else {
            !bounds.intersects(rect)
        }
    }
}

#[test]
fn a_rectangle_answers_exactly_as_the_0_212_0_formulas_did() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..4000 {
        let bounds = rng.rect(30, 24);
        let rect = rng.rect(40, 20);
        let (x, y) = (rng.range(-45, 45), rng.range(-45, 45));
        for inverted in [false, true] {
            let s = Selection {
                inverted,
                ..Selection::new(bounds)
            };
            assert!(s.is_rect());
            assert_eq!(
                s.coverage_at(x, y).to_bits(),
                old::coverage_at(&bounds, inverted, x, y).to_bits()
            );
            assert_eq!(s.coverage_bounds(), old::coverage_bounds(&bounds, inverted));
            assert_eq!(
                s.fully_covers(&rect),
                old::fully_covers(&bounds, inverted, &rect),
                "{bounds:?} {rect:?} inv {inverted}"
            );
            assert_eq!(
                s.fully_excludes(&rect),
                old::fully_excludes(&bounds, inverted, &rect),
                "{bounds:?} {rect:?} inv {inverted}"
            );
        }
    }
}

#[test]
fn an_ellipse_is_symmetric_about_its_centre() {
    for bounds in [
        r(0, 0, 40, 24),
        r(-7, 3, 31, 17),
        r(5, 5, 9, 9),
        r(0, 0, 6, 33),
    ] {
        let s = Selection::ellipse(bounds);
        for y in bounds.y..bounds.bottom() {
            for x in bounds.x..bounds.right() {
                let mx = bounds.x + bounds.right() - 1 - x;
                let my = bounds.y + bounds.bottom() - 1 - y;
                for (a, b) in [(mx, y), (x, my), (mx, my)] {
                    assert_eq!(
                        s.coverage_at(x, y).to_bits(),
                        s.coverage_at(a, b).to_bits(),
                        "{bounds:?} ({x},{y}) vs ({a},{b})"
                    );
                }
            }
        }
    }
}

#[test]
fn an_ellipse_area_is_pi_a_b_and_its_edge_is_fractional() {
    for (w, h) in [(100_u32, 60_u32), (64, 64), (37, 91), (200, 20)] {
        let bounds = r(10, -4, w, h);
        let s = Selection::ellipse(bounds);
        let (mut area, mut fractional, mut full) = (0.0_f64, 0_u32, 0_u32);
        for y in bounds.y - 2..bounds.bottom() + 2 {
            for x in bounds.x - 2..bounds.right() + 2 {
                let c = s.coverage_at(x, y);
                assert!((0.0..=1.0).contains(&c));
                area += f64::from(c);
                if c > 0.0 && c < 1.0 {
                    fractional += 1;
                }
                if c == 1.0 {
                    full += 1;
                }
            }
        }
        let exact = std::f64::consts::PI * f64::from(w) / 2.0 * f64::from(h) / 2.0;
        assert!(
            (area - exact).abs() / exact < 0.01,
            "{w}x{h}: area {area} vs {exact}"
        );
        assert!(fractional > 0, "an antialiased edge");
        assert!(full > 0, "an interior");
        // Nothing outside the bounds, nothing in the bounds' corners.
        assert_eq!(s.coverage_at(bounds.x - 1, bounds.y + 1), 0.0);
        assert_eq!(s.coverage_at(bounds.right(), bounds.y + 1), 0.0);
        assert_eq!(s.coverage_at(bounds.x, bounds.y), 0.0, "corner texel");
        let (cx, cy) = (bounds.x + i64::from(w / 2), bounds.y + i64::from(h / 2));
        assert_eq!(s.coverage_at(cx, cy), 1.0, "centre");
    }
}

#[test]
fn inverting_an_ellipse_or_mask_is_the_exact_complement() {
    let mask =
        SelectionMask::rasterize(r(0, 0, 20, 14), |x, y| ((x * 7 + y * 3) % 11) as f32 / 10.0);
    let Ok(mask) = mask else {
        unreachable!("small mask");
    };
    for s in [
        Selection::ellipse(r(2, 2, 17, 11)),
        Selection::from_mask(Arc::new(mask)),
    ] {
        let inv = Selection {
            inverted: true,
            ..s.clone()
        };
        for y in -3..20 {
            for x in -3..25 {
                assert_eq!(
                    s.coverage_at(x, y) + inv.coverage_at(x, y),
                    1.0,
                    "({x},{y})"
                );
            }
        }
        assert_eq!(s.coverage_bounds(), Some(s.bounds));
        assert_eq!(inv.coverage_bounds(), None);
    }
}

/// Brute-force oracle: the extremes of per-texel coverage over `rect`.
fn extremes(s: &Selection, rect: &Rect) -> (f32, f32) {
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let c = s.coverage_at(x, y);
            lo = lo.min(c);
            hi = hi.max(c);
        }
    }
    (lo, hi)
}

fn check_conservative(s: &Selection, rects: &[Rect]) -> (u32, u32) {
    let (mut tight_covers, mut tight_excludes) = (0, 0);
    for rect in rects {
        if rect.is_empty() {
            continue;
        }
        let (lo, hi) = extremes(s, rect);
        if s.fully_covers(rect) {
            assert_eq!(lo, 1.0, "fully_covers lied: {s:?} {rect:?}");
            tight_covers += 1;
        }
        if s.fully_excludes(rect) {
            assert_eq!(hi, 0.0, "fully_excludes lied: {s:?} {rect:?}");
            tight_excludes += 1;
        }
    }
    (tight_covers, tight_excludes)
}

#[test]
fn ellipse_region_queries_are_conservative_and_not_vacuous() {
    let mut rng = Rng(0xDEAD_BEEF_1234_5678);
    let rects: Vec<Rect> = (0..3000).map(|_| rng.rect(40, 22)).collect();
    for bounds in [r(-8, -5, 50, 30), r(3, 3, 33, 33), r(0, 0, 7, 40)] {
        for inverted in [false, true] {
            let s = Selection {
                inverted,
                ..Selection::ellipse(bounds)
            };
            let (covers, excludes) = check_conservative(&s, &rects);
            assert!(covers > 0, "the answer `fully_covers` is sometimes true");
            assert!(
                excludes > 0,
                "the answer `fully_excludes` is sometimes true"
            );
        }
    }
    // Specific cases: a rectangle at the bounds' corner misses the
    // ellipse, one at the centre is covered, one straddling the edge
    // is neither.
    let s = Selection::ellipse(r(0, 0, 100, 100));
    assert!(s.fully_excludes(&r(0, 0, 10, 10)));
    assert!(s.fully_covers(&r(40, 40, 20, 20)));
    assert!(!s.fully_covers(&r(0, 40, 20, 20)) && !s.fully_excludes(&r(0, 40, 20, 20)));
    assert!(s.fully_excludes(&r(200, 0, 5, 5)), "outside the bounds");
}

fn blob(bounds: Rect) -> SelectionMask {
    let r2 = |x: i64, y: i64| {
        let (dx, dy) = (x - bounds.x - 20, y - bounds.y - 15);
        dx * dx + dy * dy
    };
    match SelectionMask::rasterize(bounds, |x, y| {
        let d = r2(x, y);
        if d < 100 {
            1.0
        } else if d < 169 {
            0.5
        } else {
            0.0
        }
    }) {
        Ok(m) => m,
        Err(err) => unreachable!("{err:?}"),
    }
}

#[test]
fn mask_region_queries_are_conservative_and_the_blocks_are_used() {
    let mut rng = Rng(0x0123_4567_89AB_CDEF);
    let bounds = r(5, -3, 41, 33);
    let rects: Vec<Rect> = (0..4000).map(|_| rng.rect(45, 30)).collect();
    for inverted in [false, true] {
        let s = Selection {
            inverted,
            ..Selection::from_mask(Arc::new(blob(bounds)))
        };
        let (covers, excludes) = check_conservative(&s, &rects);
        assert!(covers > 0 && excludes > 0, "{covers} {excludes}");
    }
    // A mask is exact: every texel of a rectangle is covered iff the
    // oracle says so (no conservatism lost on a mask).
    let s = Selection::from_mask(Arc::new(blob(bounds)));
    for rect in &rects {
        if rect.is_empty() {
            continue;
        }
        let (lo, hi) = extremes(&s, rect);
        assert_eq!(s.fully_covers(rect), lo == 1.0, "{rect:?}");
        assert_eq!(s.fully_excludes(rect), hi == 0.0, "{rect:?}");
    }
}

#[test]
fn a_mask_reads_back_bytes_and_zero_outside_and_validates_its_input() {
    let Ok(mask) = SelectionMask::new(r(10, 20, 3, 2), vec![0, 51, 255, 128, 1, 254]) else {
        unreachable!("valid");
    };
    assert_eq!(mask.byte_at(11, 20), 51);
    assert_eq!(mask.byte_at(12, 21), 254);
    assert_eq!(mask.byte_at(9, 20), 0);
    assert_eq!(mask.byte_at(13, 20), 0);
    assert_eq!(mask.byte_at(10, 22), 0);
    assert_eq!(mask.coverage_at(12, 20), 1.0);
    assert!((mask.coverage_at(11, 20) - 0.2).abs() < 1e-6);
    assert!(matches!(
        SelectionMask::new(r(0, 0, 3, 2), vec![0; 5]),
        Err(DocError::SelectionMaskInvalid)
    ));
    assert!(matches!(
        SelectionMask::new(r(0, 0, 0, 2), Vec::new()),
        Err(DocError::SelectionMaskInvalid)
    ));
    // The cap is checked before any allocation: no data is even needed.
    let too_big = r(0, 0, 8193, 8192);
    assert!(too_big.area_px() > MAX_SELECTION_MASK_PIXELS);
    assert!(matches!(
        SelectionMask::new(too_big, Vec::new()),
        Err(DocError::SelectionMaskTooLarge(n)) if n == too_big.area_px()
    ));
    assert!(matches!(
        SelectionMask::rasterize(too_big, |_, _| 1.0),
        Err(DocError::SelectionMaskTooLarge(_))
    ));
    // Exactly at the cap would be allowed by the size check.
    assert_eq!(r(0, 0, 8192, 8192).area_px(), MAX_SELECTION_MASK_PIXELS);
}

#[test]
fn rasterizing_an_ellipse_matches_its_coverage_to_a_byte() {
    let bounds = r(3, 4, 31, 19);
    let ellipse = Selection::ellipse(bounds);
    let Ok(mask) = SelectionMask::rasterize(bounds, |x, y| ellipse.coverage_at(x, y)) else {
        unreachable!("small");
    };
    for y in bounds.y..bounds.bottom() {
        for x in bounds.x..bounds.right() {
            assert!(
                (mask.coverage_at(x, y) - ellipse.coverage_at(x, y)).abs() <= 1.0 / 510.0 + 1e-6,
                "({x},{y})"
            );
        }
    }
}

#[test]
fn the_boundary_trace_follows_the_half_coverage_contour_and_is_cached() {
    // A 6 x 5 mask with a 2 x 3 block selected at (2, 1): its contour is
    // a 2 x 3 rectangle of perimeter 10 unit edges.
    let mut data = vec![0_u8; 30];
    for y in 1..4 {
        for x in 2..4 {
            if let Some(cell) = data.get_mut(y * 6 + x) {
                *cell = 255;
            }
        }
    }
    // A texel at exactly 127 (below one half) is outside, 128 inside.
    if let Some(cell) = data.get_mut(0) {
        *cell = 128;
    }
    if let Some(cell) = data.get_mut(1) {
        *cell = 127;
    }
    let Ok(mask) = SelectionMask::new(r(100, 50, 6, 5), data) else {
        unreachable!("valid");
    };
    let trace = mask.boundary();
    assert!(!trace.truncated);
    let total: u64 = trace.runs.iter().map(|run| u64::from(run.len)).sum();
    assert_eq!(
        total,
        10 + 4,
        "the block's 10 edges plus the lone 128 texel's 4"
    );
    // Runs are merged: the block is four runs, the lone texel four.
    assert_eq!(trace.runs.len(), 8);
    // The block's top edge: row 51 + ... local row 1 -> document y 51.
    assert!(
        trace
            .runs
            .iter()
            .any(|run| { !run.vertical && run.x == 102 && run.y == 51 && run.len == 2 })
    );
    assert!(
        trace
            .runs
            .iter()
            .any(|run| { run.vertical && run.x == 104 && run.y == 51 && run.len == 3 })
    );
    // `base` numbers the edges consecutively.
    let mut expect = 0_u64;
    let mut ordered = trace.runs.clone();
    ordered.sort_by_key(|run| run.base);
    for run in ordered {
        assert_eq!(run.base, expect);
        expect += u64::from(run.len);
    }
    assert!(std::ptr::eq(mask.boundary(), mask.boundary()), "cached");
}

#[test]
fn a_pathological_mask_traces_partly_and_says_so() {
    let side = 600_u32;
    let data: Vec<u8> = (0..side * side)
        .map(|i| {
            if (i / side).is_multiple_of(2) && (i % side).is_multiple_of(2) {
                255
            } else {
                0
            }
        })
        .collect();
    let Ok(mask) = SelectionMask::new(r(0, 0, side, side), data) else {
        unreachable!("valid");
    };
    let trace = mask.boundary();
    assert!(trace.truncated);
    assert_eq!(trace.runs.len(), MAX_TRACE_RUNS);
}

#[test]
fn selecting_an_empty_ellipse_deselects_and_shapes_are_kept() {
    let mut set = SelectionSet::new();
    set.select(Selection::ellipse(r(3, 3, 0, 9)));
    assert_eq!(set.active(), None);
    set.select(Selection::ellipse(r(3, 3, 8, 9)));
    assert!(matches!(
        set.active().map(|s| s.shape),
        Some(SelectionShape::Ellipse)
    ));
    if let Err(err) = set.invert() {
        unreachable!("{err:?}");
    }
    if let Err(err) = set.save_active("e") {
        unreachable!("{err:?}");
    }
    set.deselect();
    if let Err(err) = set.load("e") {
        unreachable!("{err:?}");
    }
    let Some(loaded) = set.active() else {
        unreachable!("loaded");
    };
    assert!(loaded.inverted && matches!(loaded.shape, SelectionShape::Ellipse));
}

#[test]
fn mask_selections_compare_by_content_and_share_cheaply() {
    let a = Arc::new(blob(r(0, 0, 41, 31)));
    let b = Arc::new(blob(r(0, 0, 41, 31)));
    assert_eq!(Selection::from_mask(a.clone()), Selection::from_mask(b));
    let cloned = Selection::from_mask(a.clone());
    let copy = cloned.clone();
    assert_eq!(Arc::strong_count(&a), 3, "a clone shares the grid");
    drop(copy);
    assert_ne!(
        Selection::from_mask(a),
        Selection::ellipse(r(0, 0, 41, 31)),
        "different shapes differ"
    );
}

// -- 0.218.0: saved-selection caps, names, packing ---------------------------

mod persist {
    use std::sync::Arc;

    use aurora_core::Rect;

    use crate::{
        DocError, MAX_SAVED_SELECTION_BYTES, MAX_SAVED_SELECTIONS, Selection, SelectionMask,
        SelectionNameError, SelectionSet, SelectionShape, validate_selection_name,
    };

    fn r(x: i64, y: i64, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn mask_selection(width: u32, height: u32) -> Selection {
        let bounds = r(1, 2, width, height);
        let data: Vec<u8> = (0..bounds.area_px())
            .map(|i| u8::try_from((i * 31) % 251).unwrap_or(0))
            .collect();
        match SelectionMask::new(bounds, data) {
            Ok(mask) => Selection::from_mask(Arc::new(mask)),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn packing_a_mask_is_lossless_and_shrinks_runs() {
        let flat = match SelectionMask::new(r(-3, 4, 200, 100), vec![255; 20_000]) {
            Ok(m) => m,
            Err(err) => unreachable!("{err:?}"),
        };
        let packed = flat.pack();
        assert!(packed.packed_len() < 1_000, "{}", packed.packed_len());
        assert_eq!(packed.bounds(), flat.bounds());
        assert!(matches!(packed.unpack(), Ok(ref m) if *m == flat));
        let noisy = mask_selection(37, 29);
        let SelectionShape::Mask(mask) = &noisy.shape else {
            unreachable!("a mask");
        };
        assert!(matches!(mask.pack().unpack(), Ok(ref m) if m == mask.as_ref()));
    }

    #[test]
    fn saved_selections_are_capped_by_count_and_by_coverage_bytes() {
        let mut set = SelectionSet::new();
        set.select(Selection::new(r(0, 0, 2, 2)));
        for i in 0..MAX_SAVED_SELECTIONS {
            assert!(set.save_active(format!("s{i}")).is_ok());
        }
        assert!(matches!(
            set.save_active("extra"),
            Err(DocError::SavedSelectionsFull)
        ));
        assert!(
            set.save_active("s3").is_ok(),
            "a replaced name is not a new one"
        );
        assert_eq!(set.saved_count(), MAX_SAVED_SELECTIONS);

        let mut set = SelectionSet::new();
        // 2 x 64 Mpx = the whole 128 MiB; a third does not fit.
        let big = mask_selection(8192, 8192);
        set.select(big);
        assert_eq!(MAX_SAVED_SELECTION_BYTES, 2 * 8192 * 8192);
        assert!(set.save_active("a").is_ok());
        assert!(set.save_active("b").is_ok());
        assert!(matches!(
            set.save_active("c"),
            Err(DocError::SavedSelectionsTooLarge)
        ));
        assert!(
            set.save_active("a").is_ok(),
            "replacing does not double count"
        );
        assert_eq!(set.saved_mask_bytes(), MAX_SAVED_SELECTION_BYTES);
    }

    #[test]
    fn from_parts_refuses_bad_and_repeated_names() {
        let sel = Selection::new(r(0, 0, 2, 2));
        let ok = SelectionSet::from_parts(None, vec![("a".to_owned(), sel.clone())]);
        assert!(ok.is_ok_and(|s| s.saved_count() == 1));
        for bad in ["", " a", "a ", "x\ty"] {
            let r = SelectionSet::from_parts(None, vec![(bad.to_owned(), sel.clone())]);
            assert!(matches!(r, Err(DocError::InvalidSelectionName)), "{bad:?}");
        }
        let dup = SelectionSet::from_parts(
            None,
            vec![("a".to_owned(), sel.clone()), ("a".to_owned(), sel)],
        );
        assert!(matches!(dup, Err(DocError::InvalidSelectionName)));
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(validate_selection_name("  Sky  "), Ok("Sky".to_owned()));
        assert_eq!(
            validate_selection_name("  "),
            Err(SelectionNameError::Empty)
        );
        assert_eq!(
            validate_selection_name("a\u{202e}b"),
            Err(SelectionNameError::InvalidCharacter)
        );
        assert!(validate_selection_name(&"é".repeat(64)).is_ok());
        assert_eq!(
            validate_selection_name(&"é".repeat(65)),
            Err(SelectionNameError::TooLong)
        );
    }
}
