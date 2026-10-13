//! The Blur and Sharpen tools (0.223.0): Photoshop's two retouching brushes
//! on one engine, [`aurora_brush::filter_dab_covered`].
//!
//! A drag is the Brush's drag (the same radius, spacing, soft footprint,
//! stroke snapshot and one undo step, "Blur Stroke" or "Sharpen Stroke")
//! whose dabs are the filter's: `Drag::Brush::filter` carries the
//! [`FilterKind`] (`None` for every other tool). Unlike the Smudge nothing
//! is carried between dabs; each dab reads the layer as the previous dabs
//! left it. The strengths are [`ToolSettings::blur_strength`] and
//! [`ToolSettings::sharpen_strength`] (1..100 %, default 50, each its own).
//!
//! They act on the **active pixel layer only** (the composite is never
//! sampled), honour the layer's locks and the active selection, and are
//! refused in Quick Mask like the other layer-painting tools. Not done:
//! sampling all layers, a separate size from the Brush's, pressure. The
//! model is a choice, not compared with Photoshop's (see `aurora-brush`'s
//! `filter_brush.rs`).

use aurora_brush::FilterKind;
use aurora_core::Rect;
use aurora_doc::Selection;

use crate::{Drag, LockRefusal, ToolSettings};

/// The filter `tool` is, `None` for any other tool.
#[must_use]
pub(crate) fn kind_of(tool: aurora_ui::Tool) -> Option<FilterKind> {
    match tool {
        aurora_ui::Tool::Blur => Some(FilterKind::Blur),
        aurora_ui::Tool::Sharpen => Some(FilterKind::Sharpen),
        _ => None,
    }
}

/// The refusal notice a filter press gets with no pixel layer to work on.
#[must_use]
pub(crate) const fn needs_pixel_layer(kind: FilterKind) -> LockRefusal {
    match kind {
        FilterKind::Blur => LockRefusal::BlurNeedsPixelLayer,
        FilterKind::Sharpen => LockRefusal::SharpenNeedsPixelLayer,
    }
}

/// The decision a Blur or Sharpen press makes (the app calls this and
/// nothing else): `has_pixel_layer` is whether the active layer is a pixel
/// layer, `locked` the layer lock's refusal for a stroke (`press_refusal`).
/// `None` lets the stroke start.
#[must_use]
pub(crate) fn press(
    kind: FilterKind,
    has_pixel_layer: bool,
    locked: Option<LockRefusal>,
) -> Option<LockRefusal> {
    if !has_pixel_layer {
        return Some(needs_pixel_layer(kind));
    }
    locked
}

/// The filter `drag` is a stroke of, `None` for any other drag.
#[must_use]
pub(crate) fn drag_filter(drag: Option<&Drag>) -> Option<FilterKind> {
    match drag {
        Some(Drag::Brush { filter, .. }) => *filter,
        _ => None,
    }
}

/// The strength a dab of `kind` uses, `0.0..=1.0`.
#[must_use]
pub(crate) fn strength_fraction(settings: &ToolSettings, kind: FilterKind) -> f32 {
    let percent = match kind {
        FilterKind::Blur => settings.blur_strength,
        FilterKind::Sharpen => settings.sharpen_strength,
    };
    f32::from(percent.clamp(1, 100)) / 100.0
}

/// The History row label of a committed stroke of `kind`.
#[must_use]
pub(crate) const fn stroke_label(kind: FilterKind) -> &'static str {
    match kind {
        FilterKind::Blur => crate::BLUR_STROKE_LABEL,
        FilterKind::Sharpen => crate::SHARPEN_STROKE_LABEL,
    }
}

/// One Blur or Sharpen dab at document point `doc_point` on the layer with
/// `bounds`, through `selection` (the document's active one): the one call
/// `App::filter_dab` stamps with, so a test drives exactly what the app
/// does. The drag's stroke snapshot captures what the dab changes. Does
/// nothing for a drag that is not a Blur or Sharpen stroke.
#[allow(clippy::too_many_arguments)]
pub(crate) fn filter_dab_through_selection(
    preserve_alpha: bool,
    store: &mut aurora_tile::TileStore,
    surface: aurora_tile::SurfaceId,
    bounds: Rect,
    doc_point: (f32, f32),
    selection: Option<Selection>,
    settings: &ToolSettings,
    drag: &mut Option<Drag>,
) -> aurora_brush::DabOutcome {
    let Some(kind) = drag_filter(drag.as_ref()) else {
        return aurora_brush::DabOutcome::default();
    };
    let local = crate::layer_local_point(bounds, doc_point);
    let cover = crate::selection_paint::layer_selection(selection, bounds);
    aurora_brush::filter_dab_covered(
        store,
        surface,
        local,
        settings.dab_radius(crate::DabTool::Brush),
        kind,
        strength_fraction(settings, kind),
        preserve_alpha,
        crate::brush_stroke_mut(drag),
        crate::selection_paint::as_coverage(cover.as_ref()),
    )
}
