//! The Crop tool's aspect-ratio presets (0.236.0): the pure geometry
//! ([`crate::crop`]'s `Ratio`, `rect_from_points_ratio`, `resized_ratio`,
//! `refit`, `nudged_ratio`), the state's ratio, and the options-bar glue
//! ([`crate::apply_tool_control_outcome`], [`crate::sync_tool_controls`],
//! the light dismiss, the palette commands). `App` needs a window, so the
//! tests drive the free functions it calls.

#![allow(
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use aurora_core::Rect;
use aurora_ui::{CropRatio, Tool};
use aurora_widgets::ActionOutcome;
use aurora_widgets::widgets::DropdownOutcome;

use crate::crop::{
    self, Arrow, CropHandle, CropState, Nudge, Press, Ratio, nudged_ratio, rect_from_points_ratio,
    refit, resized_ratio,
};
use crate::crop_app::hold_ratio;
use crate::document_session::DocumentId;

fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn ratio(w: u32, h: u32) -> Ratio {
    match Ratio::new(w, h) {
        Some(ratio) => ratio,
        None => unreachable!("a ratio"),
    }
}

/// A small deterministic generator (no timing, no platform dependence).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
}

fn presets() -> Vec<Ratio> {
    let mut all = Vec::new();
    for choice in CropRatio::ALL {
        for portrait in [false, true] {
            if let Some(r) = crop::ratio_for(choice, portrait, (1920, 1080)) {
                all.push(r);
            }
        }
    }
    all
}

// ---- the ratio ---------------------------------------------------------

#[test]
fn a_ratio_is_in_lowest_terms_and_refuses_a_zero_side() {
    let r = ratio(1920, 1080);
    assert_eq!((r.w, r.h), (16, 9));
    assert_eq!(Ratio::new(0, 5), None);
    assert_eq!(Ratio::new(5, 0), None);
    assert_eq!((ratio(7, 7).w, ratio(7, 7).h), (1, 1));
    assert_eq!(ratio(3, 2).swapped(), ratio(2, 3));
}

#[test]
fn the_derived_side_is_the_nearest_whole_pixel_halves_up_and_at_least_one() {
    let r = ratio(3, 2);
    assert_eq!(r.height_for(30), 20);
    assert_eq!(r.height_for(31), 21); // 20.67
    assert_eq!(r.height_for(32), 21); // 21.33
    assert_eq!(r.height_for(1), 1); // 0.67 -> 1
    assert_eq!(ratio(16, 9).height_for(1), 1); // 0.56 -> 1
    assert_eq!(ratio(16, 1).height_for(1), 1); // 0.06 -> floored at 1
    assert_eq!(ratio(2, 1).height_for(3), 2); // 1.5 rounds up
    assert_eq!(r.width_for(20), 30);
    assert_eq!(r.width_for(21), 32); // 31.5 rounds up
    assert_eq!(r.width_for(0), 1);
}

#[test]
fn the_choices_map_to_ratios_for_a_canvas_and_portrait_swaps_them() {
    let canvas = (1920, 1080);
    assert_eq!(crop::ratio_for(CropRatio::Free, false, canvas), None);
    assert_eq!(crop::ratio_for(CropRatio::Free, true, canvas), None);
    assert_eq!(
        crop::ratio_for(CropRatio::Original, false, canvas),
        Some(ratio(16, 9))
    );
    assert_eq!(
        crop::ratio_for(CropRatio::Original, true, canvas),
        Some(ratio(9, 16))
    );
    assert_eq!(crop::ratio_for(CropRatio::Original, false, (0, 5)), None);
    assert_eq!(
        crop::ratio_for(CropRatio::Square, false, canvas),
        Some(ratio(1, 1))
    );
    assert_eq!(
        crop::ratio_for(CropRatio::Square, true, canvas),
        Some(ratio(1, 1))
    );
    let expected = [
        (CropRatio::ThreeTwo, (3, 2)),
        (CropRatio::FourThree, (4, 3)),
        (CropRatio::FiveFour, (5, 4)),
        (CropRatio::SixteenNine, (16, 9)),
        (CropRatio::SixteenTen, (8, 5)),
    ];
    for (choice, (w, h)) in expected {
        assert_eq!(
            crop::ratio_for(choice, false, canvas),
            Some(ratio(w, h)),
            "{choice:?}"
        );
        assert_eq!(
            crop::ratio_for(choice, true, canvas),
            Some(ratio(h, w)),
            "{choice:?} portrait"
        );
    }
}

// ---- a new rectangle ---------------------------------------------------

#[test]
fn a_new_rectangle_keeps_the_ratio_and_the_press_corner_in_every_direction() {
    let canvas = (1000, 800);
    let r = ratio(3, 2);
    // Width-dominant (300 * 2 >= 150 * 3).
    assert_eq!(
        rect_from_points_ratio((100.0, 100.0), (400.0, 250.0), canvas, r),
        Some(rect(100, 100, 300, 200))
    );
    // Height-dominant (300 * 2 < 210 * 3 is false... 600 < 630): h = 210.
    assert_eq!(
        rect_from_points_ratio((100.0, 100.0), (400.0, 310.0), canvas, r),
        Some(rect(100, 100, 315, 210))
    );
    // Up and to the left: the anchor is the bottom-right corner.
    assert_eq!(
        rect_from_points_ratio((500.0, 500.0), (200.0, 350.0), canvas, r),
        Some(rect(200, 300, 300, 200))
    );
    // Up and to the right, down and to the left.
    assert_eq!(
        rect_from_points_ratio((500.0, 500.0), (800.0, 350.0), canvas, r),
        Some(rect(500, 300, 300, 200))
    );
    assert_eq!(
        rect_from_points_ratio((500.0, 500.0), (200.0, 650.0), canvas, r),
        Some(rect(200, 500, 300, 200))
    );
}

#[test]
fn the_new_rectangle_covers_the_box_to_the_pointer_when_nothing_clamps() {
    let canvas = (2000, 1500);
    let mut rng = Lcg(7);
    for r in presets() {
        for _ in 0..200 {
            let anchor = (600 + rng.next(800) as i64, 500 + rng.next(500) as i64);
            let pointer = (
                anchor.0 + rng.next(400) as i64 - 200,
                anchor.1 + rng.next(400) as i64 - 200,
            );
            let Some(got) = rect_from_points_ratio(
                (anchor.0 as f32, anchor.1 as f32),
                (pointer.0 as f32, pointer.1 as f32),
                canvas,
                r,
            ) else {
                assert_eq!(anchor, pointer, "{r:?}");
                continue;
            };
            let (left, right) = (anchor.0.min(pointer.0), anchor.0.max(pointer.0));
            let (top, bottom) = (anchor.1.min(pointer.1), anchor.1.max(pointer.1));
            assert!(got.x <= left && got.right() >= right, "{r:?} {got:?}");
            assert!(got.y <= top && got.bottom() >= bottom, "{r:?} {got:?}");
            // The anchor is a corner of the rectangle.
            assert!(got.x == anchor.0 || got.right() == anchor.0, "{got:?}");
            assert!(got.y == anchor.1 || got.bottom() == anchor.1, "{got:?}");
        }
    }
}

#[test]
fn a_rectangle_that_leaves_the_canvas_is_shrunk_with_its_ratio_never_distorted() {
    let canvas = (100, 100);
    let wide = ratio(16, 9);
    assert_eq!(
        rect_from_points_ratio((10.0, 10.0), (500.0, 50.0), canvas, wide),
        Some(rect(10, 10, 90, 51))
    );
    assert_eq!(
        rect_from_points_ratio((10.0, 10.0), (30.0, 500.0), canvas, wide),
        Some(rect(10, 10, 90, 51))
    );
    // A tall ratio is limited by the height instead.
    let tall = ratio(9, 16);
    assert_eq!(
        rect_from_points_ratio((10.0, 10.0), (500.0, 500.0), canvas, tall),
        Some(rect(10, 10, 50, 89))
    );
    // To the up-left the room is the anchor's own distance from the edge.
    assert_eq!(
        rect_from_points_ratio((40.0, 90.0), (-300.0, -300.0), canvas, ratio(1, 1)),
        Some(rect(0, 50, 40, 40))
    );
}

#[test]
fn a_press_with_no_room_on_the_chosen_side_or_no_drag_makes_no_rectangle() {
    let canvas = (100, 100);
    let r = ratio(1, 1);
    assert_eq!(
        rect_from_points_ratio((100.0, 50.0), (150.0, 60.0), canvas, r),
        None
    );
    assert_eq!(
        rect_from_points_ratio((50.0, 50.0), (50.0, 50.0), canvas, r),
        None
    );
    assert_eq!(
        rect_from_points_ratio((f32::NAN, 5.0), (10.0, 10.0), canvas, r),
        None
    );
    // A pointer on one axis only is a thin drag, still a ratio rectangle.
    assert_eq!(
        rect_from_points_ratio((10.0, 10.0), (10.0, 30.0), canvas, ratio(1, 2)),
        Some(rect(10, 10, 10, 20))
    );
}

#[test]
fn an_exact_ratio_is_hit_exactly_and_the_error_bound_holds_for_every_size() {
    let r = ratio(16, 9);
    let canvas = (4000, 4000);
    for k in 1..=40_u32 {
        let got = rect_from_points_ratio((0.0, 0.0), ((16 * k) as f32, 0.0), canvas, r);
        assert_eq!(got, Some(rect(0, 0, 16 * k, 9 * k)), "k = {k}");
        let got = rect_from_points_ratio((0.0, 0.0), (0.0, (9 * k) as f32), canvas, r);
        assert_eq!(got, Some(rect(0, 0, 16 * k, 9 * k)), "k = {k}");
    }
    // Every size of every preset: the nearest-pixel bound.
    //   2 * |w * rh - h * rw| <= max(rw, rh)         (the rounding), and
    //   |w / h - rw / rh| / (rw / rh) <= 1 / min(w, h)  (the relative error)
    // whenever neither side was floored at 1 px.
    for r in presets() {
        for dominant in 2..400_u32 {
            for horizontal in [true, false] {
                let to = if horizontal {
                    (dominant as f32, 0.0)
                } else {
                    (0.0, dominant as f32)
                };
                let Some(got) = rect_from_points_ratio((0.0, 0.0), to, canvas, r) else {
                    unreachable!("a rectangle for {to:?}")
                };
                let (w, h) = (u64::from(got.width), u64::from(got.height));
                if w.min(h) <= 1 {
                    continue;
                }
                let (rw, rh) = (u64::from(r.w), u64::from(r.h));
                let error = (w * rh).abs_diff(h * rw);
                assert!(2 * error <= rw.max(rh), "{r:?} {got:?}");
                assert!(error * w.min(h) <= h * rw, "{r:?} {got:?} relative");
            }
        }
    }
}

// ---- resizing ----------------------------------------------------------

#[test]
fn a_corner_handle_keeps_the_opposite_corner_and_the_ratio() {
    let canvas = (1000, 800);
    let r = ratio(3, 2);
    let origin = rect(200, 200, 300, 200); // right 500, bottom 400
    let got = resized_ratio(origin, CropHandle::TopLeft, (100.0, 150.0), canvas, r);
    assert_eq!(got, rect(100, 133, 400, 267));
    assert_eq!((got.right(), got.bottom()), (500, 400));
    let got = resized_ratio(origin, CropHandle::BottomRight, (650.0, 600.0), canvas, r);
    // Height-dominant here (450 * 2 < 400 * 3): the box to the pointer is
    // covered by a 600 x 400 rectangle.
    assert_eq!(got, rect(200, 200, 600, 400));
    let got = resized_ratio(origin, CropHandle::TopRight, (650.0, 100.0), canvas, r);
    assert_eq!((got.x, got.bottom()), (200, 400));
    assert_eq!(got, rect(200, 100, 450, 300));
    let got = resized_ratio(origin, CropHandle::BottomLeft, (50.0, 500.0), canvas, r);
    assert_eq!((got.right(), got.y), (500, 200));
    assert_eq!(got, rect(50, 200, 450, 300));
}

#[test]
fn the_fixed_corner_never_moves_and_the_ratio_holds_for_any_pointer() {
    let canvas = (1200, 900);
    let mut rng = Lcg(99);
    for r in presets() {
        let origin = rect(300, 250, 400, 300);
        for handle in [
            CropHandle::TopLeft,
            CropHandle::TopRight,
            CropHandle::BottomRight,
            CropHandle::BottomLeft,
        ] {
            for _ in 0..100 {
                let to = (rng.next(1300) as f32 - 50.0, rng.next(1000) as f32 - 50.0);
                let got = resized_ratio(origin, handle, to, canvas, r);
                let fixed = match handle {
                    CropHandle::TopLeft => (origin.right(), origin.bottom()),
                    CropHandle::TopRight => (origin.x, origin.bottom()),
                    CropHandle::BottomRight => (origin.x, origin.y),
                    _ => (origin.right(), origin.y),
                };
                if got == origin {
                    continue; // no room on the chosen side: left as it was
                }
                let corners = [
                    (got.x, got.y),
                    (got.right(), got.y),
                    (got.x, got.bottom()),
                    (got.right(), got.bottom()),
                ];
                assert!(corners.contains(&fixed), "{handle:?} {to:?} {got:?}");
                assert!(got.x >= 0 && got.y >= 0, "{got:?}");
                assert!(got.right() <= 1200 && got.bottom() <= 900, "{got:?}");
                let (w, h) = (u64::from(got.width), u64::from(got.height));
                if w.min(h) > 1 {
                    let error = (w * u64::from(r.h)).abs_diff(h * u64::from(r.w));
                    assert!(2 * error <= u64::from(r.w.max(r.h)), "{got:?}");
                }
            }
        }
    }
}

#[test]
fn a_corner_dragged_across_the_fixed_corner_flips_and_clamps_by_shrinking() {
    let canvas = (1000, 800);
    let r = ratio(3, 2);
    let origin = rect(200, 200, 300, 200);
    // Bottom-right handle dragged up and left past the top-left corner.
    let got = resized_ratio(origin, CropHandle::BottomRight, (50.0, 80.0), canvas, r);
    assert_eq!((got.right(), got.bottom()), (200, 200));
    assert_eq!(got, rect(20, 80, 180, 120));
    // Dragged far outside: shrunk to the canvas, same ratio.
    let got = resized_ratio(origin, CropHandle::BottomRight, (5000.0, 5000.0), canvas, r);
    assert_eq!(got, rect(200, 200, 800, 533));
    assert!(got.right() <= 1000 && got.bottom() <= 800);
    // No room on the chosen side: left as it was.
    let edge = rect(900, 200, 100, 100);
    assert_eq!(
        resized_ratio(
            edge,
            CropHandle::TopLeft,
            (1500.0, 150.0),
            canvas,
            ratio(1, 1)
        ),
        edge
    );
    // A non-finite point changes nothing.
    assert_eq!(
        resized_ratio(origin, CropHandle::TopLeft, (f32::NAN, 1.0), canvas, r),
        origin
    );
}

#[test]
fn an_edge_handle_moves_its_edge_and_derives_the_other_side_about_the_centre() {
    let canvas = (1000, 800);
    let r = ratio(3, 2);
    let origin = rect(200, 200, 300, 200); // centre y = 300, centre x = 350
    let got = resized_ratio(origin, CropHandle::Right, (650.0, 0.0), canvas, r);
    assert_eq!(got, rect(200, 150, 450, 300));
    let got = resized_ratio(origin, CropHandle::Left, (100.0, 0.0), canvas, r);
    assert_eq!(got, rect(100, 166, 400, 267));
    assert_eq!(got.right(), 500);
    let got = resized_ratio(origin, CropHandle::Bottom, (0.0, 500.0), canvas, r);
    assert_eq!(got, rect(125, 200, 450, 300));
    let got = resized_ratio(origin, CropHandle::Top, (0.0, 100.0), canvas, r);
    assert_eq!(got.bottom(), 400);
    assert_eq!(got, rect(125, 100, 450, 300));
    // The centre line stays (within half a pixel).
    for handle in [CropHandle::Left, CropHandle::Right] {
        let got = resized_ratio(origin, handle, (700.0, 0.0), canvas, r);
        let centre2 = got.y * 2 + i64::from(got.height);
        assert!((centre2 - 600).abs() <= 1, "{handle:?} {got:?}");
    }
}

#[test]
fn an_edge_handle_slides_back_inside_and_shrinks_to_the_canvas() {
    let canvas = (1000, 800);
    let r = ratio(3, 2);
    // Near the top: the centred box would stick out above, so it slides down.
    let near_top = rect(200, 0, 300, 200);
    assert_eq!(
        resized_ratio(near_top, CropHandle::Right, (800.0, 0.0), canvas, r),
        rect(200, 0, 600, 400)
    );
    // Dragged far outside: shrunk to the room, ratio kept.
    let origin = rect(200, 200, 300, 200);
    let got = resized_ratio(origin, CropHandle::Right, (9000.0, 0.0), canvas, r);
    assert_eq!(got, rect(200, 33, 800, 533));
    assert!(got.right() <= 1000);
    // A tall canvas limits an edge drag by the height.
    let got = resized_ratio(origin, CropHandle::Left, (-900.0, 0.0), (1000, 300), r);
    assert!(got.bottom() <= 300 && got.y >= 0, "{got:?}");
    assert_eq!(got.right(), 500);
}

#[test]
fn a_top_or_bottom_edge_uses_the_transposed_ratio() {
    // 16:9 landscape: dragging the bottom edge down 90 px gives 90 tall and
    // 160 wide, not 90 wide and 50 tall.
    let canvas = (2000, 1000);
    let r = ratio(16, 9);
    let origin = rect(400, 100, 320, 180);
    let got = resized_ratio(origin, CropHandle::Bottom, (0.0, 190.0), canvas, r);
    assert_eq!((got.width, got.height), (160, 90));
    assert_eq!(got.y, 100);
    let got = resized_ratio(origin, CropHandle::Top, (0.0, 20.0), canvas, r);
    assert_eq!(got.bottom(), 280);
    assert_eq!((got.width, got.height), (462, 260)); // 260 * 16 / 9 = 462.2
}

// ---- the state ---------------------------------------------------------

fn state_with(ratio: Option<Ratio>, canvas: (u32, u32)) -> CropState {
    let mut state = CropState::default();
    state.set_ratio(ratio, canvas);
    state
}

#[test]
fn a_drag_with_a_ratio_makes_and_resizes_ratio_rectangles_and_a_move_keeps_the_size() {
    let canvas = (1000, 800);
    let id = DocumentId::next();
    let now = std::time::Instant::now();
    let mut state = state_with(Some(ratio(3, 2)), canvas);
    assert_eq!(state.ratio(), Some(ratio(3, 2)));
    assert_eq!(
        state.press((100.0, 100.0), 6.0, now, id, canvas),
        Press::Started
    );
    state.drag((400.0, 250.0), canvas);
    assert_eq!(state.rect(), Some(rect(100, 100, 300, 200)));
    state.release();
    // Inside: a move keeps the size.
    let later = now + std::time::Duration::from_secs(5);
    state.press((200.0, 180.0), 6.0, later, id, canvas);
    state.drag((260.0, 200.0), canvas);
    assert_eq!(state.rect(), Some(rect(160, 120, 300, 200)));
    state.release();
    // A handle resizes with the ratio.
    let much_later = later + std::time::Duration::from_secs(5);
    state.press((460.0, 320.0), 6.0, much_later, id, canvas);
    state.drag((610.0, 420.0), canvas);
    assert_eq!(state.rect(), Some(rect(160, 120, 450, 300)));
}

#[test]
fn a_free_state_still_makes_free_rectangles() {
    let canvas = (1000, 800);
    let id = DocumentId::next();
    let mut state = state_with(None, canvas);
    state.press((100.0, 100.0), 6.0, std::time::Instant::now(), id, canvas);
    state.drag((400.0, 250.0), canvas);
    assert_eq!(state.rect(), Some(rect(100, 100, 300, 150)));
}

#[test]
fn changing_the_ratio_refits_the_rectangle_about_its_centre_keeping_the_area() {
    let canvas = (1000, 800);
    let mut state = state_with(None, canvas);
    let id = DocumentId::next();
    state.press((100.0, 100.0), 6.0, std::time::Instant::now(), id, canvas);
    state.drag((500.0, 400.0), canvas);
    state.release();
    assert_eq!(state.rect(), Some(rect(100, 100, 400, 300)));
    assert!(state.set_ratio(Some(ratio(1, 1)), canvas));
    // sqrt(120000) = 346.4 -> 346; centred on (300, 250).
    assert_eq!(state.rect(), Some(rect(127, 77, 346, 346)));
    // Back to a wide ratio: the area stays within rounding.
    assert!(state.set_ratio(Some(ratio(16, 9)), canvas));
    let Some(got) = state.rect() else {
        unreachable!("a rectangle")
    };
    let before = 346_i64 * 346;
    let after = i64::from(got.width) * i64::from(got.height);
    assert!((after - before).abs() * 100 < before * 2, "{got:?}");
    assert_eq!(
        (got.width * 9).abs_diff(got.height * 16) * 2 <= 16,
        true,
        "{got:?}"
    );
    // The centre stays within a pixel.
    assert!(
        (got.x * 2 + i64::from(got.width) - 600).abs() <= 1,
        "{got:?}"
    );
    // Free leaves the rectangle as it is.
    let kept = state.rect();
    assert!(state.set_ratio(None, canvas));
    assert_eq!(state.rect(), kept);
    // The same ratio again changes nothing.
    assert!(!state.set_ratio(None, canvas));
}

#[test]
fn a_refit_slides_inside_the_canvas_and_shrinks_when_it_cannot_fit() {
    let canvas = (100, 100);
    // A corner rectangle: the centred new box would stick out to the left.
    assert_eq!(
        refit(rect(0, 0, 100, 100), ratio(16, 9), (200, 100)),
        rect(0, 12, 133, 75)
    );
    assert_eq!(
        refit(rect(0, 0, 50, 50), ratio(16, 9), (50, 50)),
        rect(0, 11, 50, 28)
    );
    // Fits as is: the centre stays.
    assert_eq!(
        refit(rect(20, 20, 60, 60), ratio(1, 1), canvas),
        rect(20, 20, 60, 60)
    );
    // Portrait from landscape.
    let got = refit(rect(100, 100, 400, 300), ratio(2, 3), (1000, 800));
    assert!(got.height > got.width, "{got:?}");
    assert!(got.x >= 0 && got.y >= 0 && got.right() <= 1000 && got.bottom() <= 800);
}

#[test]
fn the_refit_keeps_the_area_close_for_any_rectangle_and_ratio() {
    let canvas = (3000, 2000);
    let mut rng = Lcg(5);
    for r in presets() {
        for _ in 0..100 {
            let (w, h) = (50 + rng.next(900) as u32, 50 + rng.next(700) as u32);
            let origin = rect(rng.next(1500) as i64, rng.next(900) as i64, w, h);
            let got = refit(origin, r, canvas);
            assert!(got.x >= 0 && got.y >= 0, "{got:?}");
            assert!(got.right() <= 3000 && got.bottom() <= 2000, "{got:?}");
            let before = f64::from(w) * f64::from(h);
            let after = f64::from(got.width) * f64::from(got.height);
            let side = f64::from(got.width.min(got.height));
            assert!(
                (after - before).abs() / before <= 3.0 / side,
                "{origin:?} -> {got:?} for {r:?}"
            );
        }
    }
}

#[test]
fn changing_the_ratio_ends_a_live_drag_and_cancel_keeps_the_ratio() {
    let canvas = (1000, 800);
    let id = DocumentId::next();
    let mut state = state_with(None, canvas);
    state.press((100.0, 100.0), 6.0, std::time::Instant::now(), id, canvas);
    state.drag((300.0, 250.0), canvas);
    assert!(state.dragging());
    assert!(state.set_ratio(Some(ratio(1, 1)), canvas));
    assert!(!state.dragging(), "the drag belonged to the old ratio");
    state.cancel();
    assert_eq!(state.rect(), None);
    assert_eq!(state.ratio(), Some(ratio(1, 1)), "a setting, not a gesture");
}

#[test]
fn the_app_holds_the_state_to_the_options_bars_choice_and_the_canvas() {
    let canvas = (1920, 1080);
    let mut state = CropState::default();
    assert!(!hold_ratio(&mut state, CropRatio::Free, false, canvas));
    assert!(hold_ratio(&mut state, CropRatio::ThreeTwo, false, canvas));
    assert_eq!(state.ratio(), Some(ratio(3, 2)));
    assert!(!hold_ratio(&mut state, CropRatio::ThreeTwo, false, canvas));
    assert!(hold_ratio(&mut state, CropRatio::ThreeTwo, true, canvas));
    assert_eq!(state.ratio(), Some(ratio(2, 3)));
    assert!(hold_ratio(&mut state, CropRatio::Original, false, canvas));
    assert_eq!(state.ratio(), Some(ratio(16, 9)));
    // Original follows the canvas.
    assert!(hold_ratio(
        &mut state,
        CropRatio::Original,
        false,
        (800, 600)
    ));
    assert_eq!(state.ratio(), Some(ratio(4, 3)));
    assert!(hold_ratio(&mut state, CropRatio::Free, true, canvas));
    assert_eq!(state.ratio(), None);
}

// ---- the keyboard ------------------------------------------------------

fn alt(arrow: Arrow, step: u32) -> Nudge {
    Nudge {
        arrow,
        step,
        resize: true,
    }
}

#[test]
fn an_alt_arrow_resize_keeps_the_ratio_by_rounding_the_other_side() {
    let canvas = (2000, 1000);
    let r = Some(ratio(16, 9));
    let start = rect(10, 10, 160, 90);
    assert_eq!(
        nudged_ratio(start, alt(Arrow::Right, 1), canvas, r),
        rect(10, 10, 161, 91)
    );
    assert_eq!(
        nudged_ratio(start, alt(Arrow::Down, 1), canvas, r),
        rect(10, 10, 162, 91)
    );
    assert_eq!(
        nudged_ratio(start, alt(Arrow::Left, 10), canvas, r),
        rect(10, 10, 150, 84)
    );
    assert_eq!(
        nudged_ratio(start, alt(Arrow::Up, 10), canvas, r),
        rect(10, 10, 142, 80)
    );
    // Without a ratio, or without Alt, nothing changes from the old rules.
    assert_eq!(
        nudged_ratio(start, alt(Arrow::Right, 1), canvas, None),
        rect(10, 10, 161, 90)
    );
    let moving = Nudge {
        arrow: Arrow::Right,
        step: 10,
        resize: false,
    };
    assert_eq!(
        nudged_ratio(start, moving, canvas, r),
        rect(20, 10, 160, 90)
    );
}

#[test]
fn a_ratio_resize_stops_at_the_canvas_and_never_goes_below_one_pixel() {
    let r = Some(ratio(16, 9));
    // 170 x 100 canvas: the width cannot grow past 160, the height past 90.
    assert_eq!(
        nudged_ratio(rect(10, 10, 160, 90), alt(Arrow::Right, 10), (170, 100), r),
        rect(10, 10, 160, 90)
    );
    // Shrinking to the floor.
    let tiny = nudged_ratio(rect(0, 0, 4, 2), alt(Arrow::Left, 10), (100, 100), r);
    assert!(tiny.width >= 1 && tiny.height >= 1, "{tiny:?}");
    // A tall limit: growing the width is cut by the room below.
    let got = nudged_ratio(rect(0, 80, 32, 18), alt(Arrow::Right, 10), (200, 100), r);
    assert!(got.bottom() <= 100, "{got:?}");
}

#[test]
fn the_state_nudges_with_its_ratio() {
    let canvas = (2000, 1000);
    let mut state = state_with(Some(ratio(16, 9)), canvas);
    let id = DocumentId::next();
    state.press((10.0, 10.0), 6.0, std::time::Instant::now(), id, canvas);
    state.drag((170.0, 100.0), canvas);
    state.release();
    assert_eq!(state.rect(), Some(rect(10, 10, 160, 90)));
    assert!(state.nudge(alt(Arrow::Right, 1), canvas));
    assert_eq!(state.rect(), Some(rect(10, 10, 161, 91)));
}

// ---- the options bar and the palette ----------------------------------

fn bar() -> (aurora_ui::Workspace, aurora_ui::ToolControls) {
    let scales = crate::test_workspace_scales();
    let mut workspace = aurora_ui::build_workspace(&scales);
    let controls = match aurora_ui::insert_tool_controls(
        &mut workspace.tree,
        workspace.options_bar,
        workspace.properties,
        &scales,
    ) {
        Ok(controls) => controls,
        Err(err) => unreachable!("{err:?}"),
    };
    workspace.tree.compute_layout(1600.0, 900.0);
    (workspace, controls)
}

fn commit(index: usize, id: aurora_widgets::WidgetId) -> crate::PointerOutcome {
    crate::PointerOutcome::Action(ActionOutcome::Dropdown {
        id,
        outcome: DropdownOutcome::Committed {
            index,
            changed: true,
        },
    })
}

#[test]
fn the_settings_default_to_free_landscape() {
    let settings = crate::ToolSettings::default();
    assert_eq!(settings.crop_ratio, CropRatio::Free);
    assert!(!settings.crop_portrait);
}

#[test]
fn the_dropdown_commit_and_the_portrait_toggle_edit_the_settings_and_the_sync_mirrors_them() {
    let (mut workspace, controls) = bar();
    let mut settings = crate::ToolSettings::default();
    assert!(crate::apply_tool_control_outcome(
        &mut workspace,
        Some(controls),
        Tool::Crop,
        &mut settings,
        &commit(CropRatio::SixteenNine.index(), controls.crop.ratio),
        None,
    ));
    assert_eq!(settings.crop_ratio, CropRatio::SixteenNine);
    // The same commit again changes nothing.
    assert!(!crate::apply_tool_control_outcome(
        &mut workspace,
        Some(controls),
        Tool::Crop,
        &mut settings,
        &commit(CropRatio::SixteenNine.index(), controls.crop.ratio),
        None,
    ));
    // A commit on another dropdown is not ours.
    assert!(!crate::apply_tool_control_outcome(
        &mut workspace,
        Some(controls),
        Tool::Crop,
        &mut settings,
        &commit(0, controls.crop.portrait),
        None,
    ));
    assert_eq!(settings.crop_ratio, CropRatio::SixteenNine);
    let toggled = |on| {
        crate::PointerOutcome::Action(ActionOutcome::Toggled {
            id: controls.crop.portrait,
            state: if on {
                accesskit::Toggled::True
            } else {
                accesskit::Toggled::False
            },
        })
    };
    assert!(crate::apply_tool_control_outcome(
        &mut workspace,
        Some(controls),
        Tool::Crop,
        &mut settings,
        &toggled(true),
        None,
    ));
    assert!(settings.crop_portrait);
    assert!(crate::apply_tool_control_outcome(
        &mut workspace,
        Some(controls),
        Tool::Crop,
        &mut settings,
        &toggled(false),
        None,
    ));
    assert!(!settings.crop_portrait);
    // The tool-control sync mirrors both into the widgets and shows them
    // for the Crop tool only.
    settings.crop_ratio = CropRatio::FourThree;
    settings.crop_portrait = true;
    let _ = crate::sync_tool_controls(&mut workspace, Some(controls), Tool::Crop, &settings, None);
    let selected = aurora_widgets::widgets::dropdown_state(&workspace.tree, controls.crop.ratio)
        .ok()
        .and_then(aurora_widgets::widgets::DropdownState::selected);
    assert_eq!(selected, Some(CropRatio::FourThree.index()));
    assert!(matches!(
        workspace.tree.payload(controls.crop.portrait),
        Some(aurora_widgets::widgets::WidgetKind::Checkbox(s)) if s.checked == accesskit::Toggled::True
    ));
    let hidden = |workspace: &aurora_ui::Workspace| {
        workspace
            .tree
            .accessibility(controls.crop.group)
            .is_some_and(accesskit::Node::is_hidden)
    };
    assert!(!hidden(&workspace));
    let _ = crate::sync_tool_controls(&mut workspace, Some(controls), Tool::Brush, &settings, None);
    assert!(hidden(&workspace));
}

#[test]
fn a_press_outside_the_open_ratio_list_closes_it_and_a_press_inside_keeps_it() {
    let (mut workspace, controls) = bar();
    let settings = crate::ToolSettings::default();
    let _ = crate::sync_tool_controls(&mut workspace, Some(controls), Tool::Crop, &settings, None);
    workspace.tree.compute_layout(1600.0, 900.0);
    let is_open = |workspace: &aurora_ui::Workspace| {
        aurora_widgets::widgets::dropdown_state(&workspace.tree, controls.crop.ratio)
            .is_ok_and(aurora_widgets::widgets::DropdownState::is_open)
    };
    assert!(
        aurora_widgets::widgets::set_dropdown_open(&mut workspace.tree, controls.crop.ratio, true)
            .is_ok()
    );
    workspace.tree.compute_layout(1600.0, 900.0);
    assert!(is_open(&workspace));
    let Some(bounds) = workspace.tree.bounds(controls.crop.ratio) else {
        unreachable!("laid out");
    };
    let inside = (
        (bounds.x + i64::from(bounds.width / 2)) as f32,
        (bounds.y + i64::from(bounds.height / 2)) as f32,
    );
    assert!(!crate::crop_controls_light_dismiss(
        &mut workspace.tree,
        &controls,
        inside
    ));
    assert!(is_open(&workspace));
    // The window's far corner is not the dropdown.
    assert!(crate::crop_controls_light_dismiss(
        &mut workspace.tree,
        &controls,
        (1590.0, 890.0)
    ));
    assert!(!is_open(&workspace));
    // Closed: nothing to dismiss.
    assert!(!crate::crop_controls_light_dismiss(
        &mut workspace.tree,
        &controls,
        (1590.0, 890.0)
    ));
}

/// The routed press (`route_widget_pointer`, what `App` calls) light
/// dismisses the open ratio list: a press outside the options bar's
/// controls closes it and is reported as `dismissed`.
#[test]
fn the_routed_pointer_press_light_dismisses_the_open_ratio_list() {
    let (mut workspace, controls) = bar();
    let scales = crate::test_workspace_scales();
    let settings = crate::ToolSettings::default();
    let _ = crate::sync_tool_controls(&mut workspace, Some(controls), Tool::Crop, &settings, None);
    assert!(
        aurora_widgets::widgets::set_dropdown_open(&mut workspace.tree, controls.crop.ratio, true)
            .is_ok()
    );
    workspace.tree.compute_layout(1600.0, 900.0);
    let mut focus = aurora_widgets::FocusManager::default();
    let mut click = aurora_widgets::ClickTracker::default();
    let mut gallery = None;
    let routed = crate::route_widget_pointer(
        &mut workspace,
        &mut focus,
        &mut gallery,
        None,
        Some(&controls),
        &mut click,
        &scales,
        false,
        aurora_widgets::PointerPhase::Down,
        (1590.0, 890.0),
        aurora_widgets::shortcut::Modifiers::default(),
        &mut aurora_widgets::NoTextHit,
    );
    assert!(routed.dismissed);
    assert!(
        aurora_widgets::widgets::dropdown_state(&workspace.tree, controls.crop.ratio)
            .is_ok_and(|state| !state.is_open())
    );
}

#[test]
fn the_palette_lists_and_resolves_every_ratio_and_the_orientation_swap() {
    let commands = crate::palette_commands();
    let mut file_dialog = crate::tests::FakeFileDialog::default();
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    let mut focus = aurora_widgets::FocusManager::default();
    for choice in CropRatio::ALL {
        let id = crop::ratio_command_id(choice);
        let entry = commands.iter().find(|c| c.id == id);
        assert_eq!(
            entry.map(|c| c.title.as_str()),
            Some(format!("Crop Ratio: {}", choice.label()).as_str()),
            "{choice:?}"
        );
        assert_eq!(
            crate::activate_command(&mut workspace, &mut focus, id, &mut file_dialog),
            Some(crate::ActivatedCommand::SetCropRatio(choice)),
            "{choice:?}"
        );
    }
    assert!(
        commands
            .iter()
            .any(|c| c.id == crop::COMMAND_SWAP_ORIENTATION && c.title == "Crop: Swap Orientation")
    );
    assert_eq!(
        crate::activate_command(
            &mut workspace,
            &mut focus,
            crop::COMMAND_SWAP_ORIENTATION,
            &mut file_dialog
        ),
        Some(crate::ActivatedCommand::SwapCropOrientation)
    );
    let mut ids: Vec<_> = CropRatio::ALL
        .iter()
        .map(|c| crop::ratio_command_id(*c))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), CropRatio::ALL.len(), "ids are unique");
}
