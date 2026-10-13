//! The Clone Stamp (0.221.0), Photoshop's tool in **aligned** mode.
//!
//! `Alt`+click sets the **source point** on the active pixel layer (the
//! composite is never sampled: Photoshop's "Current Layer"). The first
//! plain press after a source is set fixes the **offset**
//! `round(source - press)` per axis (whole texels, so a clone is a copy and
//! never a resample); it then persists across strokes until a new
//! `Alt`+click, a change of the active layer, a document switch, New, Open
//! or close clears it ([`CloneStamp::clear`], [`CloneStamp::observe_layer`]).
//! A drag paints [`aurora_brush::clone_dab_covered`] dabs with the Brush's
//! radius, spacing and soft footprint; its source is the layer as it was
//! when the stroke began (see that function), so a stroke never re-clones its
//! own output. One stroke is one undo step, "Clone Stamp Stroke".
//!
//! Not done: the non-aligned mode, sampling all layers or the composite, a
//! source marker or crosshair overlay (no token exists for one), opacity,
//! flow and a source-offset readout. A press with no source does nothing but
//! show the calm notice "Alt-click to set a clone source."

use aurora_core::Rect;
use aurora_doc::{LayerId, Selection};

use crate::{Drag, LockRefusal, ToolSettings};

/// The Clone Stamp's source and aligned offset.
#[derive(Debug, Clone, Default)]
pub(crate) struct CloneStamp {
    source: Option<Source>,
    offset: Option<(i64, i64)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Source {
    layer: LayerId,
    /// The `Alt`+click point, in document space.
    point: (f32, f32),
}

impl CloneStamp {
    /// Sets the source: the layer is the active pixel layer, `point` is in
    /// document space. Forgets the aligned offset, so the next stroke fixes
    /// a new one.
    pub(crate) fn set_source(&mut self, layer: LayerId, point: (f32, f32)) {
        self.source = Some(Source { layer, point });
        self.offset = None;
    }

    /// Forgets the source and the offset.
    pub(crate) fn clear(&mut self) {
        self.source = None;
        self.offset = None;
    }

    /// Clears everything when the active layer is not the source's.
    pub(crate) fn observe_layer(&mut self, active: Option<LayerId>) {
        if let Some(source) = self.source
            && active != Some(source.layer)
        {
            self.clear();
        }
    }

    /// The source point and layer, if one is set.
    #[cfg(test)]
    pub(crate) fn source(&self) -> Option<(LayerId, (f32, f32))> {
        self.source.map(|s| (s.layer, s.point))
    }

    /// The aligned offset a stroke starting at `press` (document space) on
    /// `layer` clones with: fixed at the first stroke after the source was
    /// set and kept after that. `None` when there is no source for `layer`.
    pub(crate) fn begin_stroke(&mut self, layer: LayerId, press: (f32, f32)) -> Option<(i64, i64)> {
        let source = self.source.filter(|s| s.layer == layer)?;
        if self.offset.is_none() {
            self.offset = Some(offset_between(source.point, press));
        }
        self.offset
    }
}

/// `round(source - press)` per axis; a non-finite coordinate gives `0`.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn offset_between(source: (f32, f32), press: (f32, f32)) -> (i64, i64) {
    let axis = |s: f32, p: f32| {
        let d = (s - p).round();
        if d.is_finite() { d as i64 } else { 0 }
    };
    (axis(source.0, press.0), axis(source.1, press.1))
}

/// What a Clone Stamp press resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Press {
    /// `Alt` was held: the source is set, no stroke starts.
    SourceSet,
    /// No stroke and a calm notice.
    Refused(LockRefusal),
    /// A stroke starts, cloning at this offset.
    Stroke((i64, i64)),
}

/// The one decision a Clone Stamp primary press makes (the app calls this
/// and nothing else): `alt` is `Alt` held at the press, `layer` the active
/// pixel layer (`None` for a group, an adjustment layer or none).
pub(crate) fn press(
    state: &mut CloneStamp,
    alt: bool,
    layer: Option<LayerId>,
    doc_point: (f32, f32),
) -> Press {
    let Some(layer) = layer else {
        return Press::Refused(LockRefusal::CloneNeedsPixelLayer);
    };
    if alt {
        state.set_source(layer, doc_point);
        return Press::SourceSet;
    }
    match state.begin_stroke(layer, doc_point) {
        Some(offset) => Press::Stroke(offset),
        None => Press::Refused(LockRefusal::CloneNeedsSource),
    }
}

/// Stores the press's offset in a freshly begun Clone Stamp drag.
pub(crate) fn set_drag_offset(drag: &mut Option<Drag>, offset: (i64, i64)) {
    if let Some(Drag::Brush { clone, .. }) = drag
        && clone.is_some()
    {
        *clone = Some(offset);
    }
}

/// The offset of a Clone Stamp drag, `None` for any other drag.
#[must_use]
pub(crate) fn drag_offset(drag: Option<&Drag>) -> Option<(i64, i64)> {
    match drag {
        Some(Drag::Brush { clone, .. }) => *clone,
        _ => None,
    }
}

/// One Clone Stamp dab at document point `doc_point` on the layer with
/// `bounds`, through `selection` (the document's active one): the one call
/// `App::clone_dab` stamps with, so a test drives exactly what the app does.
/// The drag's own stroke snapshot captures what the dab changes and is where
/// the stroke-start source is read from.
#[allow(clippy::too_many_arguments)]
pub(crate) fn clone_dab_through_selection(
    preserve_alpha: bool,
    store: &mut aurora_tile::TileStore,
    surface: aurora_tile::SurfaceId,
    bounds: Rect,
    doc_point: (f32, f32),
    offset: (i64, i64),
    selection: Option<Selection>,
    settings: &ToolSettings,
    drag: &mut Option<Drag>,
) -> aurora_brush::DabOutcome {
    let local = crate::layer_local_point(bounds, doc_point);
    let cover = crate::selection_paint::layer_selection(selection, bounds);
    aurora_brush::clone_dab_covered(
        store,
        surface,
        local,
        settings.dab_radius(crate::DabTool::Brush),
        offset,
        preserve_alpha,
        crate::brush_stroke_mut(drag),
        crate::selection_paint::as_coverage(cover.as_ref()),
    )
}
