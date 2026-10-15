//! The Quick Select tool (0.238.0), the document half. Pure functions over
//! the live drag, the selection and the tile store (never `App`), so a test
//! drives them without a window; `lib.rs` owns the pointer plumbing
//! (`Drag::QuickSelect`, `App::quick_select_stamp`).
//!
//! **A stroke.** A press on the active pixel layer starts the gesture in the
//! mode [`crate::selection_modes::gesture_mode`] gives (the options bar's
//! button, else `Shift` adds and `Alt` subtracts): in New mode the press
//! deselects, in a combining mode the selection stays. The pointer's path is
//! walked with the Brush's dab spacing and radius (`ToolSettings::
//! brush_radius`, `advance_segment`); each stamp grows a region of the layer
//! ([`aurora_brush::quick_select_stamp`]: the footprint, then a connected
//! flood of texels within the shared Tolerance of the footprint's mean
//! colour, inside a window of a few radii) into the stroke's `grown` set. The
//! live selection is the antialiased mask of `grown` (the Magic Wand's
//! [`crate::lasso_tools::region_selection`]) combined with the selection held
//! at the press by the gesture's mode, so with Alt the same shape is
//! subtracted. It is rebuilt after each batch of stamps, backing off after a
//! slow rebuild (the Lasso's rule); the release rebuilds exactly. One undo
//! step, "Quick Select" (the combining modes' own labels in a combining
//! mode, "Deselect" when nothing was selected); Escape restores the selection
//! held at the press. A stroke ended any other way (a tab switch, an undo)
//! keeps the selection as it was last built.
//!
//! **Bounds.** The selected set holds at most [`aurora_brush::MAX_FILL_TILES`]
//! tiles; a stamp that would exceed it stops the stroke's growth, latches
//! the calm "too large" notice and keeps what was selected so far.

use aurora_core::Rect;
use aurora_doc::{LayerKind, LayerTree, Selection, SelectionMode, SelectionSet};
use aurora_tile::{SurfaceId, TileStore};

use crate::lasso_tools::{WandRefused, backoff_until, region_selection};
use crate::selection_modes::{ModeGesture, initial_current, press_selection};
use crate::{Drag, LockRefusal, ToolSettings};

/// The History row of a Quick Select stroke.
pub(crate) const QUICK_SELECT_LABEL: &str = "Quick Select";

/// The layer a stroke reads, fixed at the press.
#[derive(Debug, Clone, Copy)]
struct QuickLayer {
    surface: SurfaceId,
    bounds: Rect,
}

/// One live Quick Select stroke (`Drag::QuickSelect`).
#[derive(Debug)]
pub(crate) struct QuickDrag {
    /// The last pointer position stamped to, document space.
    pub(crate) last_doc: (f32, f32),
    /// The distance carried toward the next stamp.
    pub(crate) carry: f32,
    /// What was selected when the stroke began: the step's "before", and
    /// what Escape puts back.
    pub(crate) before: Option<Selection>,
    /// The selection built so far: the step's "after".
    pub(crate) current: Option<Selection>,
    /// The mode and the selection held at the press.
    pub(crate) modal: ModeGesture,
    layer: Option<QuickLayer>,
    grown: aurora_brush::QuickRegion,
    /// `grown` has changed since the live selection was last built.
    dirty: bool,
    not_before: Option<std::time::Instant>,
    /// A stamp hit the tile cap: no more growth this stroke.
    capped: bool,
    /// A tile read failure was already logged this stroke.
    warned: bool,
}

impl QuickDrag {
    /// A stroke at `last_doc`, with no layer and nothing selected yet.
    pub(crate) fn new(last_doc: (f32, f32)) -> Self {
        Self {
            last_doc,
            carry: 0.0,
            before: None,
            current: None,
            modal: ModeGesture::replace(),
            layer: None,
            grown: aurora_brush::QuickRegion::new(aurora_brush::MAX_FILL_TILES),
            dirty: false,
            not_before: None,
            capped: false,
            warned: false,
        }
    }

    /// Makes the next live rebuilds wait until `at`, as after a slow one.
    #[cfg(test)]
    pub(crate) fn defer_rebuilds_until(&mut self, at: std::time::Instant) {
        self.not_before = Some(at);
    }
}

/// What a stamp did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StampOutcome {
    /// Texels were newly selected.
    Grew,
    /// Nothing new (the footprint was already selected, or off the layer).
    Unchanged,
    /// The tile cap stopped the stroke's growth.
    Capped,
    /// A tile could not be read.
    Failed,
}

/// Why a Quick Select press cannot start: the active layer is not a pixel
/// layer. Otherwise the layer's surface and document rectangle.
///
/// # Errors
///
/// [`LockRefusal::QuickSelectNeedsPixelLayer`].
fn press_layer(
    layers: &LayerTree,
    active: Option<aurora_doc::LayerId>,
) -> Result<QuickLayer, LockRefusal> {
    let refusal = LockRefusal::QuickSelectNeedsPixelLayer;
    let id = active.ok_or(refusal)?;
    let Some(LayerKind::Pixel { bounds }) = layers.kind(id).cloned() else {
        return Err(refusal);
    };
    let surface = layers.surface_id(id).ok_or(refusal)?;
    Ok(QuickLayer { surface, bounds })
}

/// Whether a press would be refused for want of a pixel layer.
#[must_use]
pub(crate) fn press_refusal(
    layers: &LayerTree,
    active: Option<aurora_doc::LayerId>,
) -> Option<LockRefusal> {
    press_layer(layers, active).err()
}

/// Begins the gesture on the live `Drag::QuickSelect` in `slot` (made by
/// `begin_drag`): fixes the layer, applies the mode's press rule to the
/// selection (New deselects) and remembers the selection held. A press with
/// no pixel layer drops the drag and says why.
///
/// # Errors
///
/// The refusal, with the drag already dropped.
pub(crate) fn begin(
    slot: &mut Option<Drag>,
    layers: &LayerTree,
    active: Option<aurora_doc::LayerId>,
    selection: &mut SelectionSet,
    mode: SelectionMode,
) -> Result<(), LockRefusal> {
    let layer = match press_layer(layers, active) {
        Ok(layer) => layer,
        Err(why) => {
            *slot = None;
            return Err(why);
        }
    };
    if let Some(Drag::QuickSelect(quick)) = slot.as_mut() {
        let (gesture, began_with) = press_selection(selection, mode);
        quick.before = began_with;
        quick.current = initial_current(&gesture);
        quick.modal = gesture;
        quick.layer = Some(layer);
    }
    Ok(())
}

/// The stamp centres along the pointer's move to `current_doc`: the
/// Brush's spacing for the Brush radius `radius`.
pub(crate) fn advance(
    quick: &mut QuickDrag,
    current_doc: (f32, f32),
    radius: f32,
) -> Vec<(f32, f32)> {
    let step = aurora_brush::dab_step(radius, aurora_brush::DEFAULT_SPACING);
    let (dabs, carry) =
        aurora_brush::advance_segment(quick.last_doc, current_doc, quick.carry, step);
    quick.last_doc = current_doc;
    quick.carry = carry;
    dabs
}

/// One stamp at `doc_point` (Brush radius, shared Tolerance), if `slot`
/// holds a Quick Select stroke.
pub(crate) fn stamp(
    slot: &mut Option<Drag>,
    store: &mut TileStore,
    doc_point: (f32, f32),
    settings: &ToolSettings,
) -> StampOutcome {
    let Some(Drag::QuickSelect(quick)) = slot.as_mut() else {
        return StampOutcome::Unchanged;
    };
    let Some(layer) = quick.layer else {
        return StampOutcome::Unchanged;
    };
    if quick.capped {
        return StampOutcome::Capped;
    }
    let stamp = aurora_brush::QuickStamp {
        centre: crate::layer_local_point(layer.bounds, doc_point),
        radius: settings.dab_radius(crate::DabTool::Brush),
        tolerance: settings.tolerance,
    };
    match aurora_brush::quick_select_stamp(
        store,
        layer.surface,
        (layer.bounds.width, layer.bounds.height),
        &stamp,
        &mut quick.grown,
    ) {
        Ok(true) => {
            quick.dirty = true;
            StampOutcome::Grew
        }
        Ok(false) => StampOutcome::Unchanged,
        Err(aurora_brush::FillError::TooLarge { .. }) => {
            // Part of the stamp may have merged: build what there is.
            quick.capped = true;
            quick.dirty = true;
            quick.modal.refused.set(true);
            StampOutcome::Capped
        }
        Err(aurora_brush::FillError::Tile(err)) => {
            if !quick.warned {
                quick.warned = true;
                tracing::warn!(?err, "the Quick Select could not read a tile");
            }
            StampOutcome::Failed
        }
    }
}

/// Rebuilds the live selection from the stroke's selected set when it has
/// changed: skipped (until the next call) while a back-off after a slow
/// rebuild lasts unless `force`; the release forces. Sets `current`.
pub(crate) fn refresh(
    slot: &mut Option<Drag>,
    selection: &mut SelectionSet,
    document_size: (u32, u32),
    force: bool,
) {
    let Some(Drag::QuickSelect(quick)) = slot.as_mut() else {
        return;
    };
    if !quick.dirty {
        return;
    }
    let started = std::time::Instant::now();
    if !force && quick.not_before.is_some_and(|t| started < t) {
        return;
    }
    let Some(layer) = quick.layer else {
        return;
    };
    let shape = match quick.grown.to_region() {
        Ok(Some(region)) => match region_selection(&region, layer.bounds, document_size) {
            Ok(shape) => Some(shape),
            Err(WandRefused::Nothing) => None,
            Err(WandRefused::Notice(_)) => {
                quick.modal.refused.set(true);
                return;
            }
        },
        Ok(None) => None,
        Err(_) => {
            quick.modal.refused.set(true);
            return;
        }
    };
    if quick.modal.place(selection, shape.as_ref()) {
        quick.current = selection.active();
    }
    quick.dirty = false;
    quick.not_before = backoff_until(started, started.elapsed());
}

/// A pointer release: the final selection is built exactly.
pub(crate) fn release(
    ending: &mut Option<Drag>,
    selection: &mut SelectionSet,
    document_size: (u32, u32),
) {
    refresh(ending, selection, document_size, true);
}

/// The History row of a finished stroke: the tool's own, or "Deselect" when
/// it ended with nothing selected.
pub(crate) fn step_label(selected: bool) -> &'static str {
    if selected {
        QUICK_SELECT_LABEL
    } else {
        "Deselect"
    }
}
