//! A document smaller than the canvas area is centred at every zoom (task
//! 144): the placement every frame path uses, the pointer-to-document
//! mapping it must agree with, painting under the cursor, the Fit and 100 %
//! commands, and a resize.

#![allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]

use aurora_ui::{CanvasView, Tool, ViewFit};

use crate::zoomed_view::{self, Placement, ViewZoom, apply_view_zoom};
use crate::{
    Drag, PanBounds, PointerButton, ToolSettings, apply_view_fit, begin_drag, continue_drag,
    shifted_view_rect, view_fit,
};

fn fit(canvas: (f32, f32), doc: (u32, u32), scale: f32) -> ViewFit {
    ViewFit {
        canvas,
        doc,
        scale_factor: scale,
    }
}

fn view_with(canvas: (f32, f32), doc: (u32, u32), scale: f32) -> CanvasView {
    let mut view = CanvasView::new();
    view.set_fit(Some(fit(canvas, doc, scale)));
    view
}

/// The frame's placement of `view`, as `redraw` computes it.
fn placed(view: &CanvasView, canvas: (f32, f32), scale: f32) -> (Placement, f32) {
    let zoom = view.zoom() * scale;
    let physical = (
        (canvas.0 * scale).round() as u32,
        (canvas.1 * scale).round() as u32,
    );
    let origin = view.to_document((0.0, 0.0));
    (zoomed_view::placement(origin, zoom, physical), zoom)
}

fn bounds(canvas: (f32, f32)) -> PanBounds {
    PanBounds {
        min_doc: (0.0, 0.0),
        max_doc: (
            aurora_gpu::TileResidency::MAX_DOC_ORIGIN_PX,
            aurora_gpu::TileResidency::MAX_DOC_ORIGIN_PX,
        ),
        canvas_size: canvas,
    }
}

// ------------------------------------------------------------ placement

#[test]
fn a_view_inside_the_document_is_not_shifted() {
    let p = zoomed_view::placement((120.5, 40.25), 2.0, (1920, 1080));
    assert_eq!(p.origin, (120.5, 40.25));
    assert_eq!(p.shift_px, (0, 0));
    assert_eq!(p.viewport_px, (1920, 1080));
}

#[test]
fn a_view_left_of_the_document_starts_the_viewport_where_the_document_does() {
    // Origin -300 doc px at 2x: the document starts 600 physical pixels in.
    let p = zoomed_view::placement((-300.0, -50.0), 2.0, (1920, 1080));
    assert_eq!(p.origin, (0.0, 0.0), "the atlas never samples negatively");
    assert_eq!(p.shift_px, (600, 100));
    assert_eq!(p.viewport_px, (1320, 980));
    // One axis only.
    let p = zoomed_view::placement((-300.0, 10.0), 1.0, (1000, 700));
    assert_eq!(
        (p.origin, p.shift_px, p.viewport_px),
        ((0.0, 10.0), (300, 0), (700, 700))
    );
}

#[test]
fn the_placement_never_leaves_an_empty_viewport_or_takes_a_bad_origin() {
    let p = zoomed_view::placement((-1.0e9, f32::NAN), 1.0, (800, 600));
    assert_eq!(p.shift_px.0, 799);
    assert_eq!(p.viewport_px.0, 1);
    assert_eq!((p.shift_px.1, p.viewport_px.1), (0, 600));
    assert_eq!(p.origin, (0.0, 0.0));
}

#[test]
fn shifted_view_rect_cuts_the_shift_off_the_area_and_never_past_it() {
    assert_eq!(
        shifted_view_rect((10.0, 20.0, 800.0, 600.0), (0, 0)),
        (10.0, 20.0, 800.0, 600.0)
    );
    assert_eq!(
        shifted_view_rect((10.0, 20.0, 800.0, 600.0), (100, 50)),
        (110.0, 70.0, 700.0, 550.0)
    );
    let (x, _, w, _) = shifted_view_rect((10.0, 20.0, 800.0, 600.0), (5000, 0));
    assert_eq!((x, w), (10.0 + 799.0, 1.0));
}

#[test]
fn the_fit_is_the_canvas_the_document_and_a_guarded_scale() {
    assert_eq!(view_fit(None, (10, 10), 2.0), None);
    let f = view_fit(Some((100.0, 50.0)), (10, 20), 2.0);
    assert_eq!(f, Some(fit((100.0, 50.0), (10, 20), 2.0)));
    assert_eq!(
        view_fit(Some((1.0, 1.0)), (1, 1), 0.0).map(|f| f.scale_factor),
        Some(1.0)
    );
}

// ------------------------------- pointer and frame use the same origin

#[test]
fn every_pixel_the_frame_draws_is_the_document_point_the_pointer_names() {
    for scale in [1.0_f32, 2.0, 1.5] {
        for (canvas, doc) in [((1000.0, 700.0), (300, 200)), ((901.0, 633.0), (101, 77))] {
            for zoom in [0.25, 0.5, 1.0, 1.5, 3.0] {
                let mut view = view_with(canvas, doc, scale);
                view.zoom_at((canvas.0 / 2.0, canvas.1 / 2.0), zoom / scale);
                if view.centred() != (true, true) {
                    continue;
                }
                let (p, zoom_phys) = placed(&view, canvas, scale);
                assert!(
                    p.shift_px.0 > 0 && p.shift_px.1 > 0,
                    "centred means shifted"
                );
                let extent = (
                    (doc.0 as f32 * zoom_phys).round() as u32,
                    (doc.1 as f32 * zoom_phys).round() as u32,
                );
                // The first and last physical pixel the document covers.
                for (px, py) in [
                    (p.shift_px.0, p.shift_px.1),
                    (p.shift_px.0 + extent.0 / 2, p.shift_px.1 + extent.1 / 2),
                    (p.shift_px.0 + extent.0 - 1, p.shift_px.1 + extent.1 - 1),
                ] {
                    // What the atlas samples at that pixel's centre.
                    let drawn = (
                        p.origin.0 + ((px - p.shift_px.0) as f32 + 0.5) / zoom_phys,
                        p.origin.1 + ((py - p.shift_px.1) as f32 + 0.5) / zoom_phys,
                    );
                    let named =
                        view.to_document(((px as f32 + 0.5) / scale, (py as f32 + 0.5) / scale));
                    assert!(
                        (drawn.0 - named.0).abs() < 2e-3 && (drawn.1 - named.1).abs() < 2e-3,
                        "scale {scale} zoom {zoom} canvas {canvas:?}: drawn {drawn:?} named {named:?}"
                    );
                    assert!(drawn.0 >= 0.0 && drawn.0 <= doc.0 as f32 && drawn.1 <= doc.1 as f32);
                }
            }
        }
    }
}

#[test]
fn painting_lands_under_the_cursor_on_a_small_document() {
    // A stroke on a 300 x 200 document in a 1000 x 700 area at 0.5x, 1x and
    // 3x: the dabs continue_drag returns are drawn, through the frame's
    // placement, at the pointer.
    let canvas = (1000.0, 700.0);
    for zoom in [0.5_f32, 1.0, 3.0] {
        let mut view = view_with(canvas, (300, 200), 1.0);
        view.zoom_at((canvas.0 / 2.0, canvas.1 / 2.0), zoom);
        assert_eq!(view.centred(), (true, true), "zoom {zoom}");
        let (p, zoom_phys) = placed(&view, canvas, 1.0);
        let start = (p.shift_px.0 as f32 + 5.0, p.shift_px.1 as f32 + 5.0);
        let Some(mut drag) = begin_drag(Tool::Brush, PointerButton::Primary, start, &view, None)
        else {
            unreachable!("a brush press starts a drag");
        };
        let mut selection = aurora_doc::SelectionSet::new();
        let end = (start.0 + 40.0, start.1 + 30.0);
        let dabs = continue_drag(
            &mut drag,
            end,
            &mut view,
            &mut selection,
            &ToolSettings::default(),
            bounds(canvas),
        );
        assert!(!dabs.is_empty(), "zoom {zoom}");
        // The last dab is at most one spacing behind the pointer; the
        // first is one step along. Each, drawn, is on the stroke's line.
        for dab in dabs {
            let shown = (
                p.shift_px.0 as f32 + (dab.0 - p.origin.0) * zoom_phys,
                p.shift_px.1 as f32 + (dab.1 - p.origin.1) * zoom_phys,
            );
            let t = ((shown.0 - start.0) / (end.0 - start.0)).clamp(0.0, 1.0);
            let on_line = (
                start.0 + t * (end.0 - start.0),
                start.1 + t * (end.1 - start.1),
            );
            assert!(
                (shown.0 - on_line.0).abs() < 0.6 && (shown.1 - on_line.1).abs() < 0.6,
                "zoom {zoom}: dab {dab:?} drawn at {shown:?}, stroke at {on_line:?}"
            );
        }
        assert_eq!(
            view.centred(),
            (true, true),
            "the stroke did not move the view"
        );
    }
}

#[test]
fn a_drag_with_the_hand_cannot_move_a_small_document() {
    let canvas = (1000.0, 700.0);
    let mut view = view_with(canvas, (300, 200), 1.0);
    let before = view.pan();
    let Some(mut drag) = begin_drag(
        Tool::Pan,
        PointerButton::Primary,
        (500.0, 300.0),
        &view,
        None,
    ) else {
        unreachable!("the hand starts a drag");
    };
    let mut selection = aurora_doc::SelectionSet::new();
    let _ = continue_drag(
        &mut drag,
        (200.0, 100.0),
        &mut view,
        &mut selection,
        &ToolSettings::default(),
        bounds(canvas),
    );
    assert_eq!(view.pan(), before);
}

// ------------------------------------------------- Fit and 100 % commands

#[test]
fn fit_centres_the_document_and_leaves_no_pan_to_scroll() {
    let mut rng = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move |lo: u32, hi: u32| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        lo + (rng % u64::from(hi - lo + 1)) as u32
    };
    for _ in 0..300 {
        let canvas = (next(200, 2200) as f32, next(150, 1300) as f32);
        let doc = (next(8, 6000), next(8, 6000));
        let scale = match next(0, 2) {
            0 => 1.0_f32,
            1 => 2.0,
            _ => 1.5,
        };
        let mut view = view_with(canvas, doc, scale);
        view.zoom_at((17.0, 9.0), 2.0);
        view.pan_by((-33.0, -21.0));
        apply_view_zoom(&mut view, ViewZoom::Fit, canvas, doc, scale);
        assert_eq!(
            view.centred(),
            (true, true),
            "canvas {canvas:?} doc {doc:?} zoom {}",
            view.zoom()
        );
        let centre = view.to_screen((doc.0 as f32 / 2.0, doc.1 as f32 / 2.0));
        assert!(
            (centre.0 - canvas.0 / 2.0).abs() <= 0.5 / scale + 1e-2
                && (centre.1 - canvas.1 / 2.0).abs() <= 0.5 / scale + 1e-2,
            "{centre:?} in {canvas:?}"
        );
        // It fills the limiting axis (to within a hundredth of a percent).
        let fill =
            (doc.0 as f32 * view.zoom() / canvas.0).max(doc.1 as f32 * view.zoom() / canvas.1);
        assert!(fill > 0.999 || view.zoom() >= 63.9, "fill {fill}");
    }
}

#[test]
fn one_hundred_percent_centres_a_small_document_and_is_unchanged_for_a_large_one() {
    let canvas = (1000.0, 700.0);
    for scale in [1.0_f32, 2.0] {
        // Small: centred at one image pixel per physical pixel.
        let mut small = view_with(canvas, (300, 200), scale);
        small.zoom_at((10.0, 10.0), 0.3);
        apply_view_zoom(
            &mut small,
            ViewZoom::ActualPixels,
            canvas,
            (300, 200),
            scale,
        );
        assert_eq!(small.zoom(), 1.0 / scale);
        assert_eq!(small.centred(), (true, true));
        let (p, zoom_phys) = placed(&small, canvas, scale);
        assert_eq!(zoom_phys, 1.0);
        assert!(p.shift_px.0 > 0 && p.shift_px.1 > 0);
        // Large: the same bits as a view that centres nothing.
        let mut with = view_with(canvas, (4000, 3000), scale);
        let mut without = CanvasView::new();
        for v in [&mut with, &mut without] {
            v.zoom_at((300.0, 200.0), 2.5);
            v.pan_by((-120.0, -80.0));
            apply_view_zoom(v, ViewZoom::ActualPixels, canvas, (4000, 3000), scale);
        }
        // The app clamps the near edge after every zoom; the fitted view
        // already holds the document over the area (task 144), which the
        // unfitted one reaches only through that clamp.
        without.clamp_pan_to_minimum((0.0, 0.0));
        assert_eq!(with.centred(), (false, false));
        assert_eq!(with.zoom().to_bits(), without.zoom().to_bits());
        // Equal as numbers: a clamp may leave `-0.0` where the bound leaves `0.0`.
        assert_eq!(with.pan(), without.pan());
    }
}

// --------------------------------------------------------------- resize

#[test]
fn a_resize_keeps_a_small_document_centred() {
    let doc = (300, 200);
    let mut view = CanvasView::new();
    for canvas in [
        (1000.0, 700.0),
        (1400.0, 900.0),
        (640.0, 480.0),
        (2500.0, 1500.0),
    ] {
        apply_view_fit(&mut view, None, None, Some(fit(canvas, doc, 1.0)));
        let centre = view.to_screen((150.0, 100.0));
        assert!(
            (centre.0 - canvas.0 / 2.0).abs() <= 0.5 && (centre.1 - canvas.1 / 2.0).abs() <= 0.5,
            "{canvas:?} {centre:?}"
        );
    }
}

#[test]
fn an_unchanged_fit_changes_nothing_and_a_resize_mid_stroke_paints_nothing() {
    let canvas = (1000.0, 700.0);
    let mut view = view_with(canvas, (300, 200), 1.0);
    let snapshot = view;
    apply_view_fit(&mut view, None, None, Some(fit(canvas, (300, 200), 1.0)));
    assert_eq!(view, snapshot);

    let pointer = (500.0, 350.0);
    let Some(mut drag) = begin_drag(Tool::Brush, PointerButton::Primary, pointer, &view, None)
    else {
        unreachable!("a brush press starts a drag");
    };
    // The area grows under a held stroke: the centre moves 100 px right.
    apply_view_fit(
        &mut view,
        Some(&mut drag),
        Some(pointer),
        Some(fit((1200.0, 700.0), (300, 200), 1.0)),
    );
    let mut selection = aurora_doc::SelectionSet::new();
    let dabs = continue_drag(
        &mut drag,
        pointer,
        &mut view,
        &mut selection,
        &ToolSettings::default(),
        bounds((1200.0, 700.0)),
    );
    assert!(
        dabs.is_empty(),
        "a still pointer must not paint: {} dabs",
        dabs.len()
    );
    assert!(matches!(drag, Drag::Brush { .. }));
}

// ---------------------------------------------------------- source pins

/// `redraw` and the event handlers need a window, so what they must call is
/// pinned in the source (the pure parts are tested above and on the GPU).
#[test]
fn the_frame_and_the_events_use_the_same_centred_view() {
    let source = include_str!("lib.rs");
    let production = source
        .split("\n#[cfg(test)]\nmod tests {")
        .next()
        .unwrap_or(source);
    assert!(
        production.matches("self.sync_view_fit();").count() >= 6,
        "a resize, a frame, a pointer move, a press, a wheel step and a view command each sync the fit"
    );
    assert!(
        production.contains("zoomed_view::placement("),
        "the 1:1 atlas is placed through the shared placement"
    );
    assert!(
        production.contains("shifted_view_rect(rect, atlas_shift)")
            && production.contains("atlas_shift = placed.shift_px;"),
        "and drawn into the viewport that starts where the document does"
    );
    assert!(
        production.contains("placed.origin,") && production.contains("placed.viewport_px,"),
        "from the clamped origin, into the shrunk viewport"
    );
    assert!(
        !production.contains("fn apply_zoomed_out_centring"),
        "the zoomed-out-only centring is gone: the view centres at every zoom"
    );
}

// ------------------------- a wheel zoom under a live drag, small document

fn wheel(steps: f32) -> winit::event::MouseScrollDelta {
    winit::event::MouseScrollDelta::LineDelta(0.0, steps)
}

#[test]
#[allow(clippy::too_many_lines)]
fn a_wheel_zoom_mid_drag_on_a_small_document_leaves_the_fixed_references_alone() {
    let canvas = (1000.0, 700.0);
    let mut layers = aurora_doc::LayerTree::new();
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 300,
        height: 200,
    };
    let Ok(layer) = layers.add_pixel_layer("layer", bounds, None) else {
        unreachable!("a layer fits");
    };
    for scale in [1.0_f32, 2.0] {
        for steps in [2.0_f32, -3.0] {
            let anchor = (200.0, 150.0);
            let mut view = view_with(canvas, (300, 200), scale);
            view.zoom_at((500.0, 350.0), 1.0);
            let start_screen = (480.0, 330.0);

            // Marquee.
            let Some(mut drag) = begin_drag(
                Tool::MarqueeSelect,
                PointerButton::Primary,
                start_screen,
                &view,
                None,
            ) else {
                unreachable!("a marquee press starts a drag");
            };
            let before = crate::drag_reference(&drag);
            crate::apply_scroll_zoom(
                &mut view,
                Some(&mut drag),
                anchor,
                wheel(steps),
                bounds_for(canvas),
            );
            assert_eq!(
                crate::drag_reference(&drag),
                before,
                "marquee, scale {scale} steps {steps}"
            );
            assert_eq!(view.centred(), (true, true), "the document stayed small");
            view.zoom_at((500.0, 350.0), 1.0);

            // Move.
            let Some(mut drag) = begin_drag(
                Tool::Move,
                PointerButton::Primary,
                start_screen,
                &view,
                Some((layer, bounds)),
            ) else {
                unreachable!("a move press starts a drag");
            };
            let before = crate::drag_reference(&drag);
            crate::apply_scroll_zoom(
                &mut view,
                Some(&mut drag),
                anchor,
                wheel(steps),
                bounds_for(canvas),
            );
            assert_eq!(
                crate::drag_reference(&drag),
                before,
                "move, scale {scale} steps {steps}"
            );
            view.zoom_at((500.0, 350.0), 1.0);

            // Lasso: every vertex of the path stays.
            let Some(mut drag) = begin_drag(
                Tool::Lasso,
                PointerButton::Primary,
                start_screen,
                &view,
                None,
            ) else {
                unreachable!("a lasso press starts a drag");
            };
            let mut selection = aurora_doc::SelectionSet::new();
            let _ = continue_drag(
                &mut drag,
                (500.0, 340.0),
                &mut view,
                &mut selection,
                &ToolSettings::default(),
                bounds_for(canvas),
            );
            let points_before = match &drag {
                Drag::Lasso { points, .. } => points.clone(),
                _ => unreachable!("a lasso drag"),
            };
            crate::apply_scroll_zoom(
                &mut view,
                Some(&mut drag),
                anchor,
                wheel(steps),
                bounds_for(canvas),
            );
            match &drag {
                Drag::Lasso { points, .. } => {
                    assert_eq!(points, &points_before, "lasso, scale {scale}");
                }
                _ => unreachable!("a lasso drag"),
            }
            view.zoom_at((500.0, 350.0), 1.0);

            // Gradient: the start stays, the end follows the pointer.
            let Some(mut drag) = begin_drag(
                Tool::Gradient,
                PointerButton::Primary,
                start_screen,
                &view,
                None,
            ) else {
                unreachable!("a gradient press starts a drag");
            };
            let _ = continue_drag(
                &mut drag,
                anchor,
                &mut view,
                &mut selection,
                &ToolSettings::default(),
                bounds_for(canvas),
            );
            let start_before = match &drag {
                Drag::Gradient { start_doc, .. } => *start_doc,
                _ => unreachable!("a gradient drag"),
            };
            crate::apply_scroll_zoom(
                &mut view,
                Some(&mut drag),
                anchor,
                wheel(steps),
                bounds_for(canvas),
            );
            match &drag {
                Drag::Gradient {
                    start_doc,
                    current_doc,
                } => {
                    assert_eq!(*start_doc, start_before, "gradient start, scale {scale}");
                    let under = view.to_document(anchor);
                    assert!(
                        (current_doc.0 - under.0).abs() < 1e-3
                            && (current_doc.1 - under.1).abs() < 1e-3,
                        "the end stays under the pointer: {current_doc:?} vs {under:?}"
                    );
                }
                _ => unreachable!("a gradient drag"),
            }
        }
    }
}

fn bounds_for(canvas: (f32, f32)) -> PanBounds {
    bounds(canvas)
}

#[test]
fn a_wheel_zoom_mid_stroke_on_a_small_document_paints_no_stray_line() {
    let canvas = (1000.0, 700.0);
    for scale in [1.0_f32, 2.0] {
        for steps in [2.0_f32, -3.0] {
            let mut view = view_with(canvas, (300, 200), scale);
            let pointer = (330.0, 260.0);
            let Some(mut drag) =
                begin_drag(Tool::Brush, PointerButton::Primary, pointer, &view, None)
            else {
                unreachable!("a brush press starts a drag");
            };
            let mut selection = aurora_doc::SelectionSet::new();
            let settings = ToolSettings::default();
            // Move the pointer a little, as a stroke does.
            let _ = continue_drag(
                &mut drag,
                (332.0, 262.0),
                &mut view,
                &mut selection,
                &settings,
                bounds(canvas),
            );
            // The pointer rests while the wheel zooms about it.
            crate::apply_scroll_zoom(
                &mut view,
                Some(&mut drag),
                (332.0, 262.0),
                wheel(steps),
                bounds(canvas),
            );
            let dabs = continue_drag(
                &mut drag,
                (332.0, 262.0),
                &mut view,
                &mut selection,
                &settings,
                bounds(canvas),
            );
            assert!(
                dabs.is_empty(),
                "scale {scale} steps {steps}: {} stray dabs",
                dabs.len()
            );
        }
    }
}
