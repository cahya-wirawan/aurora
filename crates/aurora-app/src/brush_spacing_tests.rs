//! Which tools lay dense dabs (0.226.1): the soft Brush on a layer alone.
//! Every other dab tool, and the Brush in Quick Mask, keeps the quarter-radius
//! spacing it had, dab for dab; the Brush's pointer events are capped and
//! carry correctly. The pixel-level beading checks are in `aurora-brush`'s
//! `brush_spacing_tests`.

#![allow(clippy::float_cmp, clippy::cast_precision_loss)]

use aurora_brush::{
    BRUSH_SPACING, DEFAULT_SPACING, advance_segment, brush_dab_cap, brush_segment_step, dab_step,
};
use aurora_core::Rect;
use aurora_ui::{CanvasView, Tool};

use crate::brush_spacing::{self, DabSpacing};
use crate::{DabTool, Drag, PanBounds, PointerButton, ToolSettings, begin_drag, continue_drag};

const SPACING_TOOLS: [Tool; 10] = [
    Tool::Brush,
    Tool::Pencil,
    Tool::CloneStamp,
    Tool::Smudge,
    Tool::Blur,
    Tool::Sharpen,
    Tool::Dodge,
    Tool::Burn,
    Tool::Sponge,
    Tool::Eraser,
];

#[allow(clippy::unnecessary_wraps)]
fn layer() -> Option<(aurora_doc::LayerId, Rect)> {
    Some((
        aurora_doc::LayerId::from_raw(7),
        Rect {
            x: 0,
            y: 0,
            width: 4000,
            height: 4000,
        },
    ))
}

fn press(tool: Tool, with_layer: bool) -> Drag {
    let view = CanvasView::new();
    let drag = begin_drag(
        tool,
        PointerButton::Primary,
        (0.0, 0.0),
        &view,
        if with_layer { layer() } else { None },
    );
    match drag {
        Some(drag) => drag,
        None => unreachable!("{tool:?} begins a dab drag"),
    }
}

fn bounds() -> PanBounds {
    PanBounds {
        min_doc: (0.0, 0.0),
        max_doc: (
            aurora_gpu::TileResidency::MAX_DOC_ORIGIN_PX,
            aurora_gpu::TileResidency::MAX_DOC_ORIGIN_PX,
        ),
        canvas_size: (0.0, 0.0),
    }
}

/// Moves `drag` to `to` (document = canvas point at the default view) and
/// returns the dabs the event placed.
fn move_to(drag: &mut Drag, to: (f32, f32), settings: &ToolSettings) -> Vec<(f32, f32)> {
    let mut view = CanvasView::new();
    let mut selection = aurora_doc::SelectionSet::new();
    continue_drag(drag, to, &mut view, &mut selection, settings, bounds())
}

fn radius_of(tool: Tool, settings: &ToolSettings) -> f32 {
    if tool == Tool::Eraser {
        settings.dab_radius(DabTool::Eraser)
    } else {
        settings.dab_radius(DabTool::Brush)
    }
}

#[test]
fn only_the_soft_brush_on_a_layer_gets_the_dense_spacing() {
    for tool in SPACING_TOOLS {
        let drag = press(tool, true);
        let want = if tool == Tool::Brush {
            DabSpacing::BrushDense
        } else {
            DabSpacing::Standard
        };
        assert_eq!(brush_spacing::spacing_of(&drag), want, "{tool:?}");
        let fraction = brush_spacing::spacing_of(&drag).fraction();
        if tool == Tool::Brush {
            assert_eq!(fraction, BRUSH_SPACING);
        } else {
            assert_eq!(fraction, 0.25, "{tool:?} keeps a quarter radius");
            assert_eq!(fraction, DEFAULT_SPACING);
        }
    }
    // The constants themselves.
    assert_eq!(DabSpacing::Standard.fraction(), 0.25);
    assert_eq!(DabSpacing::BrushDense.fraction(), 0.03);
}

#[test]
fn the_brush_in_quick_mask_keeps_the_old_spacing() {
    // Quick Mask hands `begin_drag` no layer (`quick_mask::drag_layer`), so
    // the stroke has no snapshot; `stamp_toward` accumulates per dab, so
    // denser dabs would only make it heavier.
    for tool in [Tool::Brush, Tool::Pencil, Tool::Eraser] {
        let drag = press(tool, false);
        assert!(
            matches!(
                drag,
                Drag::Brush { stroke: None, .. } | Drag::Eraser { stroke: None, .. }
            ),
            "{tool:?}"
        );
        assert_eq!(
            brush_spacing::spacing_of(&drag),
            DabSpacing::Standard,
            "{tool:?}"
        );
    }
    // And the production gate that produces that drag.
    assert!(crate::quick_mask::drag_layer(true, Tool::Brush, layer()).is_none());
    assert!(crate::quick_mask::drag_layer(false, Tool::Brush, layer()).is_some());
}

#[test]
fn non_dab_drags_use_the_standard_spacing() {
    let view = CanvasView::new();
    for tool in [Tool::Move, Tool::Pan, Tool::MarqueeSelect, Tool::Eyedropper] {
        if let Some(drag) = begin_drag(tool, PointerButton::Primary, (0.0, 0.0), &view, layer()) {
            assert_eq!(brush_spacing::spacing_of(&drag), DabSpacing::Standard);
        }
    }
}

#[test]
fn every_other_tool_places_exactly_the_dabs_it_did_before() {
    let settings = ToolSettings::default();
    for tool in SPACING_TOOLS {
        if tool == Tool::Brush {
            continue;
        }
        let mut drag = press(tool, true);
        let radius = radius_of(tool, &settings);
        let step = dab_step(radius, 0.25);
        // Two events, the second reaching well past one step.
        let mut got = move_to(&mut drag, (step * 0.4, 0.0), &settings);
        got.extend(move_to(&mut drag, (step * 7.3, 0.0), &settings));
        let (want, _) = advance_segment((0.0, 0.0), (step * 7.3, 0.0), 0.0, step);
        assert_eq!(got.len(), want.len(), "{tool:?}");
        assert_eq!(got.len(), 7, "{tool:?}: a dab every quarter radius");
        for (g, w) in got.iter().zip(&want) {
            assert!(
                (g.0 - w.0).abs() < 1e-3 && g.1 == w.1,
                "{tool:?}: {g:?} vs {w:?}"
            );
        }
        // No cap on these tools: a long event places every dab it always did.
        let mut drag = press(tool, true);
        let far = step * 1000.0;
        assert_eq!(
            move_to(&mut drag, (far, 0.0), &settings).len(),
            1000,
            "{tool:?}"
        );
    }
}

#[test]
fn the_brush_in_quick_mask_places_exactly_the_dabs_it_did_before() {
    let settings = ToolSettings::default();
    let radius = settings.dab_radius(DabTool::Brush);
    let step = dab_step(radius, 0.25);
    let mut drag = press(Tool::Brush, false);
    let dabs = move_to(&mut drag, (step * 1000.0, 0.0), &settings);
    assert_eq!(dabs.len(), 1000, "uncapped, a dab per quarter radius");
}

#[test]
fn the_layer_brush_lays_dense_dabs_and_caps_one_event() {
    let settings = ToolSettings::default();
    let radius = settings.dab_radius(DabTool::Brush);
    let dense = dab_step(radius, BRUSH_SPACING);
    let cap = brush_dab_cap(radius);

    // A short event: every dense step, none dropped.
    let mut drag = press(Tool::Brush, true);
    let dabs = move_to(&mut drag, (dense * 10.5, 0.0), &settings);
    assert_eq!(dabs.len(), 10);
    for (i, d) in dabs.iter().enumerate() {
        assert!((d.0 - dense * (i + 1) as f32).abs() < 1e-3, "{d:?}");
    }
    assert!(dense < radius * 0.05, "far denser than the old quarter");

    // A long flick: at most the cap, evenly spread to the pointer.
    let mut drag = press(Tool::Brush, true);
    let flick = dense * cap as f32 * 5.0;
    assert!(
        flick / (cap as f32) < dab_step(radius, 0.25),
        "inside the range where the cap, not the old step, binds"
    );
    let dabs = move_to(&mut drag, (flick, 0.0), &settings);
    assert!(dabs.len() <= cap, "{} > {cap}", dabs.len());
    assert!(dabs.len() + 1 >= cap);
    let last = dabs.last().map_or(0.0, |d| d.0);
    assert!(
        flick - last <= flick / cap as f32 + 1e-2,
        "reaches the pointer"
    );
    match drag {
        Drag::Brush {
            last_doc, carry, ..
        } => {
            assert_eq!(last_doc, (flick, 0.0));
            assert!(carry >= 0.0 && carry <= flick / cap as f32 + 1e-2);
        }
        other => unreachable!("{other:?}"),
    }
}

#[test]
fn the_layer_brush_carries_spacing_across_many_small_events() {
    let settings = ToolSettings::default();
    let radius = settings.dab_radius(DabTool::Brush);
    let dense = dab_step(radius, BRUSH_SPACING);
    // One event versus many 0.37 px events over the same 40 px.
    let end = 40.0_f32.min(dense * brush_dab_cap(radius) as f32 * 0.9);
    let mut one = press(Tool::Brush, true);
    let whole = move_to(&mut one, (end, 0.0), &settings);
    let mut many = press(Tool::Brush, true);
    let mut pieces = Vec::new();
    let mut x = 0.0_f32;
    while x < end {
        x = (x + 0.37).min(end);
        pieces.extend(move_to(&mut many, (x, 0.0), &settings));
    }
    assert_eq!(pieces.len(), whole.len());
    for (a, b) in pieces.iter().zip(&whole) {
        assert!((a.0 - b.0).abs() < 2e-3 && a.1 == b.1, "{a:?} vs {b:?}");
    }
    // Every event was a short one: the plain dense step each time.
    assert_eq!(brush_segment_step(radius, 0.0, 0.37), dense);
}

#[test]
fn event_step_uses_the_dense_step_only_for_the_brush_spacing() {
    let (from, to) = ((0.0, 0.0), (10.0, 0.0));
    assert_eq!(
        brush_spacing::event_step(DabSpacing::Standard, 24.0, from, to, 0.0),
        6.0
    );
    assert_eq!(
        brush_spacing::event_step(DabSpacing::Standard, 24.0, from, (1.0e6, 0.0), 0.0),
        6.0,
        "the standard step is never widened"
    );
    assert_eq!(
        brush_spacing::event_step(DabSpacing::BrushDense, 24.0, from, to, 0.0),
        dab_step(24.0, BRUSH_SPACING)
    );
    let long = brush_spacing::event_step(DabSpacing::BrushDense, 24.0, from, (500.0, 0.0), 0.0);
    assert!((long - 500.0 / brush_dab_cap(24.0) as f32).abs() < 1e-2);
    // A huge event is never coarser than the old quarter-radius step.
    let huge = brush_spacing::event_step(DabSpacing::BrushDense, 24.0, from, (1.0e6, 0.0), 0.0);
    assert_eq!(huge, 6.0, "capped at the old spacing, not widened past it");
    let small = brush_spacing::event_step(DabSpacing::BrushDense, 4.0, from, (1000.0, 0.0), 0.0);
    assert_eq!(small, 1.0, "r=4 never coarser than the old 1 px step");
}

#[test]
fn a_brush_stroke_is_still_one_undo_step_labelled_brush_stroke() {
    // The stroke's snapshot, label and commit path are untouched: only the
    // dab positions changed. The label is still the one the commit reads.
    let src = include_str!("lib.rs");
    assert!(src.contains("\"Brush Stroke\""));
    assert!(src.contains("\"Eraser Stroke\""));
    // A layer stroke holds exactly one snapshot, however many dabs.
    let mut drag = press(Tool::Brush, true);
    let settings = ToolSettings::default();
    let dabs = move_to(&mut drag, (30.0, 0.0), &settings);
    assert!(dabs.len() > 30);
    match drag {
        Drag::Brush {
            stroke: Some(s), ..
        } => assert!(s.is_empty(), "no dab was stamped yet"),
        other => unreachable!("{other:?}"),
    }
}
