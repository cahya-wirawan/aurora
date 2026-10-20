//! Centring a document smaller than the canvas area (task 144): the view
//! centres it on each axis at every zoom, continuously, and an axis the
//! document overflows behaves exactly as the view did before.

#![allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]

use super::{CanvasView, MAX_ZOOM, ViewFit, centred_axis_pan};

fn fit(canvas: (f32, f32), doc: (u32, u32), scale_factor: f32) -> ViewFit {
    ViewFit {
        canvas,
        doc,
        scale_factor,
    }
}

fn view_with(canvas: (f32, f32), doc: (u32, u32), scale_factor: f32) -> CanvasView {
    let mut view = CanvasView::new();
    view.set_fit(Some(fit(canvas, doc, scale_factor)));
    view
}

fn scale_of(i: u32) -> f32 {
    match i % 3 {
        0 => 1.0,
        1 => 2.0,
        _ => 1.5,
    }
}

/// xorshift, so the differential runs are reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1_u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    fn int(&mut self, lo: u32, hi: u32) -> u32 {
        lo + (self.next() % u64::from(hi - lo + 1)) as u32
    }
}

// ----------------------------------------------------- the pure rule

#[test]
fn a_document_not_larger_than_the_area_is_centred_and_a_larger_one_is_not() {
    // 400 px in a 1000 px area: 300 px either side.
    assert_eq!(centred_axis_pan(1.0, 1000.0, 400, 1.0), Some(300.0));
    // Exactly as large: centred at 0 (nothing to pan either way).
    assert_eq!(centred_axis_pan(1.0, 1000.0, 1000, 1.0), Some(0.0));
    // One pixel larger: not centred.
    assert_eq!(centred_axis_pan(1.0, 1000.0, 1001, 1.0), None);
    // Zoom is part of the extent.
    assert_eq!(centred_axis_pan(2.0, 1000.0, 400, 1.0), Some(100.0));
    assert_eq!(centred_axis_pan(3.0, 1000.0, 400, 1.0), None);
}

#[test]
fn the_centred_position_is_a_whole_physical_pixel() {
    // (1000 - 401) / 2 = 299.5 logical; at 2x that is physical 599: a whole
    // physical pixel, 299.5 logical.
    assert_eq!(centred_axis_pan(1.0, 1000.0, 401, 2.0), Some(299.5));
    // At 1x the half pixel rounds (half away from zero) to 300.
    assert_eq!(centred_axis_pan(1.0, 1000.0, 401, 1.0), Some(300.0));
    // 1.5x: (1000 - 400.5)/2 = 299.75 logical = 449.625 physical -> 450.
    let pan = centred_axis_pan(1.0, 1000.0, 400, 1.5).unwrap_or(f32::NAN);
    assert_eq!((pan * 1.5).fract(), 0.0, "pan {pan}");
    // A degenerate scale reads as 1.
    assert_eq!(
        centred_axis_pan(1.0, 1000.0, 401, 0.0),
        centred_axis_pan(1.0, 1000.0, 401, 1.0)
    );
    assert_eq!(
        centred_axis_pan(1.0, 1000.0, 401, f32::NAN),
        centred_axis_pan(1.0, 1000.0, 401, 1.0)
    );
}

#[test]
fn a_non_finite_size_is_never_centred() {
    assert_eq!(centred_axis_pan(f32::INFINITY, 1000.0, 400, 1.0), None);
    assert_eq!(centred_axis_pan(1.0, f32::NAN, 400, 1.0), None);
    assert_eq!(centred_axis_pan(f32::NAN, 1000.0, 400, 1.0), None);
}

// ------------------------------------------- the view with and without

#[test]
fn a_view_with_no_fit_never_centres() {
    let mut view = CanvasView::new();
    assert_eq!(view.centred(), (false, false));
    assert_eq!(view.fit(), None);
    view.pan_by((-12.0, -7.0));
    assert_eq!(view.pan(), (-12.0, -7.0));
}

#[test]
fn a_small_document_is_centred_at_the_opening_view() {
    // The reported bug: opened at 1:1 the document sat at the corner and
    // jumped to the middle when zoomed out.
    let view = view_with((1000.0, 700.0), (400, 300), 1.0);
    assert_eq!(view.zoom(), 1.0);
    assert_eq!(view.centred(), (true, true));
    assert_eq!(view.pan(), (300.0, 200.0));
    assert_eq!(view.to_screen((0.0, 0.0)), (300.0, 200.0));
    assert_eq!(view.to_screen((400.0, 300.0)), (700.0, 500.0));
}

#[test]
fn each_axis_centres_on_its_own() {
    // Wide but short: x overflows, y is centred.
    let mut view = view_with((1000.0, 700.0), (3000, 300), 1.0);
    assert_eq!(view.centred(), (false, true));
    assert_eq!(view.pan(), (0.0, 200.0));
    // Panning moves x only.
    view.pan_by((-50.0, -50.0));
    assert_eq!(view.pan(), (-50.0, 200.0));
    // Tall but narrow: the other way round.
    let mut view = view_with((1000.0, 700.0), (400, 3000), 1.0);
    assert_eq!(view.centred(), (true, false));
    view.pan_by((-50.0, -50.0));
    assert_eq!(view.pan(), (300.0, -50.0));
}

#[test]
fn the_position_is_continuous_across_every_zoom_while_the_document_is_smaller() {
    // The document's centre sits at the middle of the area for every zoom
    // from 0.5 through 1.0 to the one at which it fills the area (task
    // 144's continuity: no step at 1.0, nor at the old zoomed-out floor).
    for scale in [1.0_f32, 2.0, 1.5] {
        let canvas = (1000.0, 700.0);
        let doc = (800, 500);
        let mut view = view_with(canvas, doc, scale);
        // The previous step's centre, while that step was centred.
        let mut previous: Option<(f32, f32)> = None;
        let mut zoom = 0.2_f32;
        let mut steps = 0;
        while zoom <= 1.4 {
            // A wheel step anchored far from the centre.
            view.zoom_at((canvas.0 * 0.9, canvas.1 * 0.1), zoom);
            let centre = view.to_screen((doc.0 as f32 / 2.0, doc.1 as f32 / 2.0));
            let extent = (doc.0 as f32 * zoom, doc.1 as f32 * zoom);
            if extent.0 <= canvas.0 && extent.1 <= canvas.1 {
                assert!(
                    (centre.0 - canvas.0 / 2.0).abs() <= 0.5 / scale + 1e-3
                        && (centre.1 - canvas.1 / 2.0).abs() <= 0.5 / scale + 1e-3,
                    "zoom {zoom} scale {scale}: the centre {centre:?} left the middle"
                );
            }
            if let Some(previous) = previous {
                // Never a step: the centre moves by at most the rounding of
                // a physical pixel between neighbouring zooms, even across
                // the zoom at which the document fills the area.
                assert!(
                    (centre.0 - previous.0).abs() <= 1.0 + 1e-3
                        && (centre.1 - previous.1).abs() <= 1.0 + 1e-3,
                    "zoom {zoom} scale {scale}: the centre jumped {previous:?} -> {centre:?}"
                );
            }
            previous = (extent.0 <= canvas.0 && extent.1 <= canvas.1).then_some(centre);
            zoom *= 1.01;
            steps += 1;
        }
        assert!(steps > 100);
    }
}

#[test]
fn the_position_is_continuous_across_one_to_one() {
    // The reported bug, as a number: the document's left edge at 0.99, 1.0
    // and 1.01.
    let mut view = view_with((1000.0, 700.0), (400, 300), 1.0);
    let at = |view: &mut CanvasView, zoom: f32| {
        view.zoom_at((0.0, 0.0), zoom);
        view.to_screen((0.0, 0.0))
    };
    let below = at(&mut view, 0.99);
    let exact = at(&mut view, 1.0);
    let above = at(&mut view, 1.01);
    assert!((exact.0 - below.0).abs() <= 3.0, "{below:?} -> {exact:?}");
    assert!((above.0 - exact.0).abs() <= 3.0, "{exact:?} -> {above:?}");
    assert!(
        exact.0 > 250.0 && exact.1 > 150.0,
        "at the corner: {exact:?}"
    );
}

#[test]
fn a_zoom_that_outgrows_the_area_does_so_about_its_centre() {
    // Past the zoom at which the document fills the area the left edge
    // goes negative symmetrically: the picture the pointer anchor would
    // have moved is not moved by the step that crosses.
    let canvas = (1000.0, 700.0);
    let mut view = view_with(canvas, (800, 500), 1.0);
    view.zoom_at((10.0, 10.0), 1.2);
    // 800 * 1.2 = 960 <= 1000: still centred.
    assert_eq!(view.centred(), (true, true));
    view.zoom_at((10.0, 10.0), 1.3);
    // 800 * 1.3 = 1040 > 1000: x overflows, symmetric about the centre.
    assert_eq!(view.centred(), (false, true));
    let left = view.to_screen((0.0, 0.0)).0;
    let right = view.to_screen((800.0, 0.0)).0;
    assert!((left + (right - canvas.0)).abs() < 1e-2, "{left} {right}");
    assert!(left < 0.0 && right > canvas.0);
    // The anchor works again on the overflowing axis.
    let anchor = (20.0, 30.0);
    let doc_before = view.to_document(anchor);
    view.zoom_at(anchor, 1.9);
    let doc_after = view.to_document(anchor);
    assert!(
        (doc_before.0 - doc_after.0).abs() < 1e-3,
        "{doc_before:?} {doc_after:?}"
    );
}

#[test]
fn a_centred_axis_ignores_the_zoom_anchor_and_the_other_axis_keeps_it() {
    // x overflows (anchor kept), y is centred (anchor ignored).
    let mut view = view_with((1000.0, 700.0), (3000, 300), 1.0);
    view.pan_by((-400.0, 0.0));
    let anchor = (250.0, 600.0);
    let before = view.to_document(anchor);
    view.zoom_at(anchor, 1.5);
    let after = view.to_document(anchor);
    assert!((before.0 - after.0).abs() < 1e-3, "x keeps its anchor");
    assert_eq!(view.centred(), (false, true));
    assert_eq!(
        view.pan().1,
        centred_axis_pan(1.5, 700.0, 300, 1.0).unwrap_or(f32::NAN)
    );
}

#[test]
fn pointer_and_screen_round_trip_at_many_zooms_and_sizes() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for _ in 0..400 {
        let canvas = (rng.range(200.0, 2400.0), rng.range(150.0, 1400.0));
        let doc = (rng.int(1, 4000), rng.int(1, 4000));
        let scale = scale_of(rng.int(0, 2));
        let mut view = view_with(canvas, doc, scale);
        view.zoom_at(
            (rng.range(0.0, canvas.0), rng.range(0.0, canvas.1)),
            rng.range(0.02, 8.0),
        );
        view.pan_by((rng.range(-300.0, 300.0), rng.range(-300.0, 300.0)));
        let p = (rng.range(0.0, canvas.0), rng.range(0.0, canvas.1));
        let back = view.to_screen(view.to_document(p));
        assert!(
            (back.0 - p.0).abs() < 1e-2 && (back.1 - p.1).abs() < 1e-2,
            "{p:?} -> {back:?} canvas {canvas:?} doc {doc:?} zoom {}",
            view.zoom()
        );
        let d = (rng.range(0.0, doc.0 as f32), rng.range(0.0, doc.1 as f32));
        let back = view.to_document(view.to_screen(d));
        let tol = 1e-3 * (1.0 + d.0.abs().max(d.1.abs()));
        assert!((back.0 - d.0).abs() < tol && (back.1 - d.1).abs() < tol);
    }
}

#[test]
fn growing_or_shrinking_the_area_keeps_the_document_centred() {
    let doc = (800, 500);
    let mut view = view_with((1000.0, 700.0), doc, 1.0);
    for canvas in [
        (1400.0, 900.0),
        (1000.0, 600.0),
        (900.0, 520.0),
        (3000.0, 2000.0),
    ] {
        view.set_fit(Some(fit(canvas, doc, 1.0)));
        assert_eq!(view.centred(), (true, true), "{canvas:?}");
        let centre = view.to_screen((400.0, 250.0));
        assert!(
            (centre.0 - canvas.0 / 2.0).abs() <= 0.5 && (centre.1 - canvas.1 / 2.0).abs() <= 0.5,
            "{canvas:?}: {centre:?}"
        );
    }
}

#[test]
fn an_area_that_shrinks_below_the_document_leaves_it_where_it_was_shown() {
    let doc = (800, 500);
    let mut view = view_with((1000.0, 700.0), doc, 1.0);
    let shown = view.to_screen((0.0, 0.0));
    assert_eq!(shown, (100.0, 100.0));
    // The area shrinks to 780 x 480: both axes now overflow; the document
    // stays where it was drawn (the app's pan clamps then apply).
    view.set_fit(Some(fit((780.0, 480.0), doc, 1.0)));
    assert_eq!(view.centred(), (false, false));
    assert_eq!(view.to_screen((0.0, 0.0)), shown);
}

#[test]
fn removing_the_fit_keeps_the_picture_where_it_was() {
    let mut view = view_with((1000.0, 700.0), (400, 300), 1.0);
    let shown = view.to_screen((0.0, 0.0));
    view.set_fit(None);
    assert_eq!(view.to_screen((0.0, 0.0)), shown);
}

#[test]
fn a_centred_axis_is_not_panned_by_a_drag_or_clamped_away() {
    let mut view = view_with((1000.0, 700.0), (400, 300), 1.0);
    let centred = view.pan();
    view.pan_by((120.0, -80.0));
    assert_eq!(view.pan(), centred, "nothing to pan");
    view.clamp_pan_to_minimum((0.0, 0.0));
    assert_eq!(
        view.pan(),
        centred,
        "the near bound leaves a centred axis alone"
    );
    view.clamp_pan_to_maximum((300_000.0, 300_000.0), (1000.0, 700.0));
    assert_eq!(view.pan(), centred, "and so does the far one");
    assert!(view.pan().0 > 0.0 && view.pan().1 > 0.0);
}

#[test]
fn the_clamps_still_bound_an_axis_the_document_overflows() {
    let mut view = view_with((1000.0, 700.0), (3000, 2000), 1.0);
    view.pan_by((250.0, 250.0));
    view.clamp_pan_to_minimum((0.0, 0.0));
    assert_eq!(
        view.pan(),
        (0.0, 0.0),
        "panning past the document corner is cut"
    );
    view.pan_by((-5_000_000.0, -5_000_000.0));
    view.clamp_pan_to_maximum((300_000.0, 300_000.0), (1000.0, 700.0));
    assert_eq!(view.pan(), (1000.0 - 300_000.0, 700.0 - 300_000.0));
}

// ----------------------------------- the large case is the old maths

/// The view maths as it was before task 144, verbatim.
#[derive(Clone, Copy)]
struct Old {
    pan: (f32, f32),
    zoom: f32,
    /// `Some((canvas, doc))` adds task 144's zoom bound on an overflowing
    /// axis (the one change to a large document's maths); `None` is the
    /// old view verbatim.
    bound: Option<((f32, f32), (u32, u32))>,
}

impl Old {
    fn doc_at(&self, s: (f32, f32)) -> (f32, f32) {
        (
            (s.0 - self.pan.0) / self.zoom,
            (s.1 - self.pan.1) / self.zoom,
        )
    }

    fn zoom_at(&mut self, anchor: (f32, f32), new_zoom: f32) {
        let new_zoom = new_zoom.clamp(super::MIN_ZOOM, MAX_ZOOM);
        let d = self.doc_at(anchor);
        self.zoom = new_zoom;
        self.pan = (anchor.0 - d.0 * new_zoom, anchor.1 - d.1 * new_zoom);
        if let Some((canvas, doc)) = self.bound {
            let lowest = (
                canvas.0 - doc.0 as f32 * new_zoom,
                canvas.1 - doc.1 as f32 * new_zoom,
            );
            self.pan = (
                self.pan.0.clamp(lowest.0, 0.0),
                self.pan.1.clamp(lowest.1, 0.0),
            );
        }
    }

    fn clamp_min(&mut self, min_doc: (f32, f32)) {
        let mx = -min_doc.0 * self.zoom;
        if self.pan.0 > mx {
            self.pan.0 = mx;
        }
        let my = -min_doc.1 * self.zoom;
        if self.pan.1 > my {
            self.pan.1 = my;
        }
    }

    fn clamp_max(&mut self, max_doc: (f32, f32), canvas: (f32, f32)) {
        let x = canvas.0 - max_doc.0 * self.zoom;
        if x.is_finite() && self.pan.0 < x {
            self.pan.0 = x;
        }
        let y = canvas.1 - max_doc.1 * self.zoom;
        if y.is_finite() && self.pan.1 < y {
            self.pan.1 = y;
        }
    }
}

#[test]
fn a_document_larger_than_the_area_on_both_axes_is_the_old_maths_plus_the_zoom_bound() {
    let mut rng = Rng(0x0dd_ba11_cafe_f00d);
    let mut compared = 0_u32;
    for round in 0..300 {
        let canvas = (rng.range(300.0, 2000.0), rng.range(200.0, 1400.0));
        // Larger than the area at every zoom visited (>= 1).
        let doc = (
            rng.int(canvas.0 as u32 + 1, canvas.0 as u32 * 3),
            rng.int(canvas.1 as u32 + 1, canvas.1 as u32 * 3),
        );
        let scale = scale_of(round % 3);
        let mut new = view_with(canvas, doc, scale);
        let mut old = Old {
            pan: (0.0, 0.0),
            zoom: 1.0,
            bound: Some((canvas, doc)),
        };
        for _ in 0..40 {
            match rng.int(0, 3) {
                0 => {
                    let anchor = (rng.range(0.0, canvas.0), rng.range(0.0, canvas.1));
                    let zoom = rng.range(1.0, 12.0);
                    new.zoom_at(anchor, zoom);
                    old.zoom_at(anchor, zoom);
                }
                1 => {
                    let d = (rng.range(-400.0, 400.0), rng.range(-400.0, 400.0));
                    new.pan_by(d);
                    old.pan.0 += d.0;
                    old.pan.1 += d.1;
                }
                2 => {
                    let m = (rng.range(0.0, 50.0), rng.range(0.0, 50.0));
                    new.clamp_pan_to_minimum(m);
                    old.clamp_min(m);
                }
                _ => {
                    new.clamp_pan_to_maximum((300_000.0, 300_000.0), canvas);
                    old.clamp_max((300_000.0, 300_000.0), canvas);
                }
            }
            assert_eq!(new.centred(), (false, false), "round {round}");
            assert_eq!(new.zoom().to_bits(), old.zoom.to_bits(), "round {round}");
            assert_eq!(new.pan().0.to_bits(), old.pan.0.to_bits(), "round {round}");
            assert_eq!(new.pan().1.to_bits(), old.pan.1.to_bits(), "round {round}");
            let p = (rng.range(0.0, canvas.0), rng.range(0.0, canvas.1));
            let (a, b) = (new.to_document(p), old.doc_at(p));
            assert_eq!(
                (a.0.to_bits(), a.1.to_bits()),
                (b.0.to_bits(), b.1.to_bits())
            );
            compared += 1;
        }
    }
    assert_eq!(compared, 300 * 40);
}

#[test]
fn a_view_with_no_fit_is_bit_identical_to_the_old_maths_at_every_zoom() {
    let mut rng = Rng(0xfeed_face_1234_5678);
    let mut new = CanvasView::new();
    let mut old = Old {
        pan: (0.0, 0.0),
        zoom: 1.0,
        bound: None,
    };
    for _ in 0..2000 {
        let anchor = (rng.range(0.0, 2000.0), rng.range(0.0, 1200.0));
        let zoom = rng.range(0.005, 90.0);
        new.zoom_at(anchor, zoom);
        old.zoom_at(anchor, zoom);
        let d = (rng.range(-500.0, 500.0), rng.range(-500.0, 500.0));
        new.pan_by(d);
        old.pan.0 += d.0;
        old.pan.1 += d.1;
        new.clamp_pan_to_minimum((0.0, 0.0));
        old.clamp_min((0.0, 0.0));
        assert_eq!(new.zoom().to_bits(), old.zoom.to_bits());
        assert_eq!(new.pan().0.to_bits(), old.pan.0.to_bits());
        assert_eq!(new.pan().1.to_bits(), old.pan.1.to_bits());
    }
}

#[test]
fn zooming_out_a_far_panned_document_through_its_fit_size_never_jumps() {
    // A 10,000 px document in a 1,000 px area, panned to its far end, wheel
    // zoomed out at the cursor on the area's left edge: the right edge stays
    // on the area's right edge until the document is the area's size, where
    // the (clamped) pan is exactly the centred one. No step is larger than
    // the zoom step implies.
    let canvas = (1000.0, 700.0);
    let doc = (10_000_u32, 10_000_u32);
    let mut view = view_with(canvas, doc, 1.0);
    view.pan_by((canvas.0 - 10_000.0, canvas.1 - 10_000.0));
    let mut zoom = 1.0_f32;
    let mut previous = view.to_screen((0.0, 0.0));
    let mut was_centred = false;
    while zoom > 0.04 {
        zoom *= 0.93;
        view.zoom_at((0.0, 0.0), zoom);
        let left = view.to_screen((0.0, 0.0));
        let extent = (doc.0 as f32 * view.zoom(), doc.1 as f32 * view.zoom());
        // The change in the edge's position is at most the change of the
        // extent (7 % of it) plus the pixel of centring's rounding.
        let allowed = (
            extent.0 / 0.93 * 0.07 + 1.0 + 1e-2,
            extent.1 / 0.93 * 0.07 + 1.0 + 1e-2,
        );
        assert!(
            (left.0 - previous.0).abs() <= allowed.0 && (left.1 - previous.1).abs() <= allowed.1,
            "zoom {}: the document jumped {previous:?} -> {left:?}",
            view.zoom()
        );
        if view.centred() == (false, false) {
            // Covering the area: both edges at or beyond the area's.
            assert!(left.0 <= 1e-3 && left.0 + extent.0 >= canvas.0 - 1e-3);
            assert!(left.1 <= 1e-3 && left.1 + extent.1 >= canvas.1 - 1e-3);
        } else {
            was_centred = true;
        }
        previous = left;
    }
    assert!(was_centred, "the sweep reached the centred sizes");
    assert_eq!(view.centred(), (true, true));
}

#[test]
fn the_bound_on_a_zoom_applies_to_the_zoom_only_and_not_to_a_hand_pan() {
    let mut view = view_with((1000.0, 700.0), (10_000, 10_000), 1.0);
    // A hand pan may go anywhere (the app's own clamps bound it).
    view.pan_by((-20_000.0, -20_000.0));
    assert_eq!(view.pan(), (-20_000.0, -20_000.0));
    // A zoom brings the document back over the area.
    view.zoom_at((10.0, 10.0), 1.1);
    let pan = view.pan();
    assert_eq!(pan, (1000.0 - 11_000.0, 700.0 - 11_000.0));
    // The unbounded zoom is the anchor rule alone.
    let mut free = view_with((1000.0, 700.0), (10_000, 10_000), 1.0);
    free.pan_by((-20_000.0, -20_000.0));
    free.zoom_unbounded((10.0, 10.0), 1.1);
    assert!(free.pan().0 < pan.0 && free.pan().1 < pan.1);
}

#[test]
fn the_pan_stored_for_a_centred_axis_is_never_touched() {
    // The stored pan of a centred axis is not read by anything (`pan()`,
    // `zoom_at` and `set_fit` all go through the centred value), so the
    // gating of `pan_by` and of both clamps on it cannot be seen through
    // `pan()`. This pins the stored value itself, which is what makes the
    // gating unobservable rather than missing.
    // A stored pan that is on the wrong side of every bound: x positive
    // (the near bound would zero it), y far negative (the far bound would
    // raise it), both left over from before the fit was given.
    let mut view = CanvasView::new();
    view.pan_by((50.0, -5000.0));
    view.set_fit(Some(fit((1000.0, 700.0), (400, 300), 1.0)));
    assert_eq!(view.centred(), (true, true));
    let stored = view.stored_pan();
    assert_eq!(stored, (50.0, -5000.0));
    view.pan_by((120.0, -80.0));
    assert_eq!(view.stored_pan(), stored, "pan_by");
    view.clamp_pan_to_minimum((0.0, 0.0));
    assert_eq!(view.stored_pan(), stored, "the near clamp");
    view.clamp_pan_to_maximum((300.0, 300.0), (1000.0, 700.0));
    assert_eq!(view.stored_pan(), stored, "the far clamp");
    // And on an overflowing axis the stored pan is the one in force.
    let mut wide = view_with((1000.0, 700.0), (3000, 300), 1.0);
    wide.pan_by((-30.0, -40.0));
    assert_eq!(wide.stored_pan().0, -30.0);
    assert_eq!(wide.stored_pan().1, 0.0, "the centred y axis was left");
}
