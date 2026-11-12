//! The Spot Healing tool (0.237.0) and the Healing Brush (0.314.0): Photoshop's Spot Healing Brush, simplified
//! (see `aurora-brush`'s `spot_heal.rs` for the model).
//!
//! A drag is the Brush's drag (the same radius, spacing and stroke snapshot)
//! whose dabs write **nothing**: each one only adds a hard disc to the
//! [`HealStroke`] the drag carries (`Drag::Brush::heal`). The release (or any
//! path that ends the drag through [`crate::App::commit_drag`] or a document
//! switch) heals the whole painted area at once
//! ([`finish`]) into the stroke's snapshot, and the drag then records ONE undo
//! step, "Spot Healing". There is **no live preview** while painting.
//! Escape cancels the stroke: the mask is dropped and nothing changes.
//!
//! It acts on the **active pixel layer only**, honours the layer's locks (a
//! pixels lock refuses the press with the Brush's notice; a transparency lock
//! heals the colour and keeps every alpha) and the active selection, and is
//! refused in Quick Mask. A heal that is refused (too little around the area,
//! or an area over the bound) says so in the status bar and changes nothing.
//! A stroke ended by a path that has no document to heal into (an undo, a
//! History jump, a layer command, a clipboard cut) is dropped without
//! healing.

use aurora_core::Rect;

use crate::document_session::DocumentSession;
use crate::{Drag, LockRefusal, PaintPermission, ToolSettings};

/// The History row label of a healed stroke.
pub(crate) const SPOT_HEALING_LABEL: &str = "Spot Healing";

/// The History row label of a Healing Brush stroke (0.314.0).
pub(crate) const HEALING_BRUSH_LABEL: &str = "Healing Brush";

/// The area a Spot Healing drag has painted and the radius that painted it.
#[derive(Debug, Default)]
pub(crate) struct HealStroke {
    mask: aurora_brush::HealMask,
    radius: f32,
    /// The Healing Brush's (0.314.0) aligned source offset, fixed at the
    /// press; `None` is a Spot Healing stroke.
    offset: Option<(i64, i64)>,
}

impl HealStroke {
    /// The painted area.
    #[cfg(test)]
    pub(crate) fn mask(&self) -> &aurora_brush::HealMask {
        &self.mask
    }
}

/// The History row label of the stroke a `heal` drag field describes.
#[must_use]
pub(crate) fn label(heal: Option<&HealStroke>) -> &'static str {
    if heal.is_some_and(|h| h.offset.is_some()) {
        HEALING_BRUSH_LABEL
    } else {
        SPOT_HEALING_LABEL
    }
}

/// The one decision a Healing Brush primary press makes (0.314.0): the
/// Clone Stamp's ([`crate::clone_stamp::press`]) over the Healing Brush's
/// own source state, with its own notices. `Alt`+click sets the source.
pub(crate) fn healing_press(
    state: &mut crate::clone_stamp::CloneStamp,
    alt: bool,
    layer: Option<aurora_doc::LayerId>,
    locked: Option<LockRefusal>,
    doc_point: (f32, f32),
) -> crate::clone_stamp::Press {
    crate::clone_stamp::press_with(
        state,
        alt,
        layer,
        locked,
        doc_point,
        (
            LockRefusal::SpotHealNeedsPixelLayer,
            LockRefusal::HealNeedsSource,
        ),
    )
}

/// Stores the press's offset in a freshly begun Healing Brush drag.
pub(crate) fn set_drag_offset(drag: &mut Option<Drag>, offset: (i64, i64)) {
    if let Some(Drag::Brush {
        heal: Some(stroke), ..
    }) = drag
    {
        stroke.offset = Some(offset);
    }
}

/// The decision a Spot Healing press makes: `has_pixel_layer` is whether the
/// active layer is a pixel layer, `locked` the layer lock's refusal for a
/// stroke (`press_refusal`). `None` lets the stroke start.
#[must_use]
pub(crate) fn press(has_pixel_layer: bool, locked: Option<LockRefusal>) -> Option<LockRefusal> {
    if !has_pixel_layer {
        return Some(LockRefusal::SpotHealNeedsPixelLayer);
    }
    locked
}

/// Whether `drag` is a Spot Healing stroke.
#[must_use]
pub(crate) fn is_heal(drag: Option<&Drag>) -> bool {
    matches!(drag, Some(Drag::Brush { heal: Some(_), .. }))
}

/// The painted area of a Spot Healing drag, `None` for any other drag.
#[cfg(test)]
#[must_use]
pub(crate) fn drag_stroke(drag: Option<&Drag>) -> Option<&HealStroke> {
    match drag {
        Some(Drag::Brush { heal, .. }) => heal.as_deref(),
        _ => None,
    }
}

/// One Spot Healing dab at document point `doc_point` on the layer with
/// `bounds`: adds its disc (radius from the Brush's setting) to the drag's
/// mask. Writes no pixel. Does nothing for a drag that is not a Spot Healing
/// stroke.
pub(crate) fn add_dab(
    bounds: Rect,
    doc_point: (f32, f32),
    settings: &ToolSettings,
    drag: &mut Option<Drag>,
) {
    let Some(Drag::Brush {
        heal: Some(stroke), ..
    }) = drag
    else {
        return;
    };
    let radius = settings.dab_radius(crate::DabTool::Brush);
    stroke.radius = radius;
    stroke
        .mask
        .add_dab(crate::layer_local_point(bounds, doc_point), radius);
}

/// Heals the area `drag` has painted (if it is a Spot Healing stroke that
/// painted anything) into `doc`'s active pixel layer, through the stroke's
/// snapshot and the active selection, and invalidates the composite for the
/// tiles it changed. The painted area is consumed: calling this again does
/// nothing. Returns the notice to show when the heal was refused.
pub(crate) fn finish(drag: &mut Option<Drag>, doc: &mut DocumentSession) -> Option<LockRefusal> {
    let Some(Drag::Brush {
        heal: Some(stroke),
        stroke: snapshot,
        ..
    }) = drag
    else {
        return None;
    };
    let painted = std::mem::take(&mut **stroke);
    // The emptied stroke keeps what kind it is (its History label).
    stroke.offset = painted.offset;
    if painted.mask.is_empty() && !painted.mask.overflowed() {
        return None;
    }
    let layer_id = doc.active_layer?;
    let aurora_doc::LayerKind::Pixel { bounds } = doc.layers.kind(layer_id).cloned()? else {
        return None;
    };
    let surface = doc.layers.surface_id(layer_id)?;
    // The snapshot belongs to the layer the stroke began on.
    if !snapshot.as_ref().is_some_and(|s| s.surface() == surface) {
        return None;
    }
    let preserve_alpha = match crate::paint_permission(
        crate::DabTool::Brush,
        doc.layers.lock(layer_id).unwrap_or_default(),
    ) {
        PaintPermission::Refused(why) => return Some(why),
        PaintPermission::PreserveAlpha => true,
        PaintPermission::Allowed => false,
    };
    let selection = doc.selection.active();
    let store = doc.tile_store.as_mut()?;
    let cover = crate::selection_paint::layer_selection(selection, bounds);
    let healed = match painted.offset {
        Some(offset) => aurora_brush::healing_brush_covered(
            store,
            surface,
            &painted.mask,
            painted.radius,
            offset,
            preserve_alpha,
            snapshot.as_mut(),
            crate::selection_paint::as_coverage(cover.as_ref()),
        ),
        None => aurora_brush::spot_heal_covered(
            store,
            surface,
            &painted.mask,
            painted.radius,
            preserve_alpha,
            snapshot.as_mut(),
            crate::selection_paint::as_coverage(cover.as_ref()),
        ),
    };
    match healed {
        Err(aurora_brush::HealRefusal::SourceOutside) => Some(LockRefusal::HealSourceOutside),
        Err(aurora_brush::HealRefusal::TooLarge) => Some(LockRefusal::HealTooLarge),
        Err(aurora_brush::HealRefusal::NotEnoughSurroundings) => {
            Some(LockRefusal::HealNotEnoughSurroundings)
        }
        Ok(outcome) => {
            let reference_origin = crate::composite_reference_origin(&doc.layers, Some(layer_id));
            crate::invalidate_painted_tiles(
                &mut doc.composite_cache,
                &doc.layers,
                layer_id,
                outcome.painted(),
                reference_origin,
            );
            for (tile, err) in outcome.failed() {
                tracing::warn!(?err, ?tile, "failed to heal part of a stroke");
            }
            None
        }
    }
}
