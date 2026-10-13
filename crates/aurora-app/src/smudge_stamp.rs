//! The Smudge tool (0.222.0): Photoshop's tool with finger painting off.
//!
//! A Smudge drag is the Brush's drag (the same radius, spacing, soft
//! footprint, stroke snapshot and one undo step, "Smudge Stroke") whose dabs
//! are [`aurora_brush::smudge_dab_covered`]'s. The paint the finger carries,
//! an [`aurora_brush::SmudgeCarry`], lives **in the drag**
//! (`Drag::Brush::smudge`): `begin_drag` makes a fresh, empty one at every
//! press, so nothing is carried between strokes, and it is dropped with the
//! drag when the stroke ends, the tool is released, or a document switch,
//! New, Open or close commits the live gesture. The strength is
//! [`ToolSettings::smudge_strength`] (1..100 %, default 50).
//!
//! It acts on the **active pixel layer only** (the composite is never
//! sampled), honours the layer's locks and the active selection, and is
//! refused in Quick Mask like the other layer-painting tools that edit
//! pixels another way. Not done: finger painting (starting each stroke with
//! the foreground colour), sampling all layers, a separate size from the
//! Brush's, pressure. The model itself is a choice, not compared with
//! Photoshop's (see `aurora-brush`'s `smudge.rs`).

use aurora_core::Rect;
use aurora_doc::Selection;

use crate::{Drag, LockRefusal, ToolSettings};

/// The decision a Smudge press makes (the app calls this and nothing else):
/// `has_pixel_layer` is whether the active layer is a pixel layer, `locked`
/// the layer lock's refusal for a stroke (`press_refusal`). `None` lets the
/// stroke start.
#[must_use]
pub(crate) fn press(has_pixel_layer: bool, locked: Option<LockRefusal>) -> Option<LockRefusal> {
    if !has_pixel_layer {
        return Some(LockRefusal::SmudgeNeedsPixelLayer);
    }
    locked
}

/// Whether `drag` is a Smudge stroke.
#[must_use]
pub(crate) fn is_smudge(drag: Option<&Drag>) -> bool {
    matches!(
        drag,
        Some(Drag::Brush {
            smudge: Some(_),
            ..
        })
    )
}

/// The paint a Smudge drag carries, `None` for any other drag.
#[cfg(test)]
#[must_use]
pub(crate) fn drag_carry(drag: Option<&Drag>) -> Option<&aurora_brush::SmudgeCarry> {
    match drag {
        Some(Drag::Brush { smudge, .. }) => smudge.as_deref(),
        _ => None,
    }
}

/// The strength a dab uses, `0.0..=1.0`.
#[must_use]
pub(crate) fn strength_fraction(settings: &ToolSettings) -> f32 {
    f32::from(settings.smudge_strength.clamp(1, 100)) / 100.0
}

/// One Smudge dab at document point `doc_point` on the layer with
/// `bounds`, through `selection` (the document's active one): the one call
/// `App::smudge_dab` stamps with, so a test drives exactly what the app
/// does. The drag's carry is taken out for the dab and put back, and its
/// stroke snapshot captures what the dab changes. Does nothing for a drag
/// that is not a Smudge stroke.
#[allow(clippy::too_many_arguments)]
pub(crate) fn smudge_dab_through_selection(
    preserve_alpha: bool,
    store: &mut aurora_tile::TileStore,
    surface: aurora_tile::SurfaceId,
    bounds: Rect,
    doc_point: (f32, f32),
    selection: Option<Selection>,
    settings: &ToolSettings,
    drag: &mut Option<Drag>,
) -> aurora_brush::DabOutcome {
    let Some(Drag::Brush {
        smudge: Some(carry),
        ..
    }) = drag
    else {
        return aurora_brush::DabOutcome::default();
    };
    let mut held = std::mem::take(&mut **carry);
    let local = crate::layer_local_point(bounds, doc_point);
    let cover = crate::selection_paint::layer_selection(selection, bounds);
    let outcome = aurora_brush::smudge_dab_covered(
        store,
        surface,
        local,
        settings.dab_radius(crate::DabTool::Brush),
        strength_fraction(settings),
        preserve_alpha,
        &mut held,
        crate::brush_stroke_mut(drag),
        crate::selection_paint::as_coverage(cover.as_ref()),
    );
    if let Some(Drag::Brush {
        smudge: Some(carry),
        ..
    }) = drag
    {
        **carry = held;
    }
    outcome
}
