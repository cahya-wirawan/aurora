//! The Dodge, Burn and Sponge tools (0.224.0): Photoshop's three toning
//! brushes on one engine, [`aurora_brush::tone_dab_covered`].
//!
//! A drag is the Brush's drag (the same radius, spacing, soft footprint,
//! stroke snapshot and one undo step, "Dodge Stroke", "Burn Stroke" or
//! "Sponge Stroke") whose dabs are the toning engine's: `Drag::Brush::tone`
//! carries the [`ToneKind`] (`None` for every other tool). Nothing is
//! carried between dabs; each dab reads the layer as the previous dabs left
//! it. The amounts are [`ToolSettings::dodge_exposure`],
//! [`ToolSettings::burn_exposure`] and [`ToolSettings::sponge_flow`]
//! (1..100 %, default 50, each its own); the Dodge and Burn share
//! [`ToolSettings::tone_range`] (Shadows / Midtones / Highlights, default
//! Midtones) and the Sponge has [`ToolSettings::sponge_mode`] (Desaturate or
//! Saturate, default Desaturate).
//!
//! They act on the **active pixel layer only** (the composite is never
//! sampled), honour the layer's locks and the active selection, and are
//! refused in Quick Mask like the other layer-painting tools. They never
//! change alpha, so a transparency lock changes nothing for them and they
//! never add paint to empty space. Not done: Protect Tones, sampling all
//! layers, a separate size from the Brush's, pressure. The model is a
//! choice, not compared with Photoshop's (see `aurora-brush`'s
//! `tone_brush.rs`).

use aurora_brush::ToneOp;
use aurora_core::Rect;
use aurora_doc::Selection;

use crate::{Drag, LockRefusal, ToolSettings};

/// Which of the three toning tools a drag is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToneKind {
    /// Lightens the tones of the range.
    Dodge,
    /// Darkens the tones of the range.
    Burn,
    /// Saturates or desaturates.
    Sponge,
}

/// The toning tool `tool` is, `None` for any other tool.
#[must_use]
pub(crate) fn kind_of(tool: aurora_ui::Tool) -> Option<ToneKind> {
    match tool {
        aurora_ui::Tool::Dodge => Some(ToneKind::Dodge),
        aurora_ui::Tool::Burn => Some(ToneKind::Burn),
        aurora_ui::Tool::Sponge => Some(ToneKind::Sponge),
        _ => None,
    }
}

/// The refusal notice a toning press gets with no pixel layer to work on.
#[must_use]
pub(crate) const fn needs_pixel_layer(kind: ToneKind) -> LockRefusal {
    match kind {
        ToneKind::Dodge => LockRefusal::DodgeNeedsPixelLayer,
        ToneKind::Burn => LockRefusal::BurnNeedsPixelLayer,
        ToneKind::Sponge => LockRefusal::SpongeNeedsPixelLayer,
    }
}

/// The decision a toning press makes (the app calls this and nothing
/// else): `has_pixel_layer` is whether the active layer is a pixel layer,
/// `locked` the layer lock's refusal for a stroke (`press_refusal`). `None`
/// lets the stroke start.
#[must_use]
pub(crate) fn press(
    kind: ToneKind,
    has_pixel_layer: bool,
    locked: Option<LockRefusal>,
) -> Option<LockRefusal> {
    if !has_pixel_layer {
        return Some(needs_pixel_layer(kind));
    }
    locked
}

/// The toning tool `drag` is a stroke of, `None` for any other drag.
#[must_use]
pub(crate) fn drag_tone(drag: Option<&Drag>) -> Option<ToneKind> {
    match drag {
        Some(Drag::Brush { tone, .. }) => *tone,
        _ => None,
    }
}

/// The operation a dab of `kind` does under `settings`.
#[must_use]
pub(crate) const fn op_of(settings: &ToolSettings, kind: ToneKind) -> ToneOp {
    match kind {
        ToneKind::Dodge => ToneOp::Dodge(settings.tone_range),
        ToneKind::Burn => ToneOp::Burn(settings.tone_range),
        ToneKind::Sponge => settings.sponge_mode.op(),
    }
}

/// The exposure (or flow) a dab of `kind` uses, `0.0..=1.0`.
#[must_use]
pub(crate) fn amount_fraction(settings: &ToolSettings, kind: ToneKind) -> f32 {
    let percent = match kind {
        ToneKind::Dodge => settings.dodge_exposure,
        ToneKind::Burn => settings.burn_exposure,
        ToneKind::Sponge => settings.sponge_flow,
    };
    f32::from(percent.clamp(1, 100)) / 100.0
}

/// The History row label of a committed stroke of `kind`.
#[must_use]
pub(crate) const fn stroke_label(kind: ToneKind) -> &'static str {
    match kind {
        ToneKind::Dodge => crate::DODGE_STROKE_LABEL,
        ToneKind::Burn => crate::BURN_STROKE_LABEL,
        ToneKind::Sponge => crate::SPONGE_STROKE_LABEL,
    }
}

/// One toning dab at document point `doc_point` on the layer with
/// `bounds`, through `selection` (the document's active one): the one call
/// `App::tone_dab` stamps with, so a test drives exactly what the app does.
/// The drag's stroke snapshot captures what the dab changes. Does nothing
/// for a drag that is not a toning stroke.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tone_dab_through_selection(
    store: &mut aurora_tile::TileStore,
    surface: aurora_tile::SurfaceId,
    bounds: Rect,
    doc_point: (f32, f32),
    selection: Option<Selection>,
    settings: &ToolSettings,
    drag: &mut Option<Drag>,
) -> aurora_brush::DabOutcome {
    let Some(kind) = drag_tone(drag.as_ref()) else {
        return aurora_brush::DabOutcome::default();
    };
    let local = crate::layer_local_point(bounds, doc_point);
    let cover = crate::selection_paint::layer_selection(selection, bounds);
    aurora_brush::tone_dab_covered(
        store,
        surface,
        local,
        settings.dab_radius(crate::DabTool::Brush),
        op_of(settings, kind),
        amount_fraction(settings, kind),
        crate::brush_stroke_mut(drag),
        crate::selection_paint::as_coverage(cover.as_ref()),
    )
}
