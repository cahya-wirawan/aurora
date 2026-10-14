//! Copy, Cut, Copy Merged, Paste, Paste in Place, Layer via Copy and Layer
//! via Cut (0.235.0): the commands, their free-function flows (tested
//! without a window) and `App`'s glue. The pixel work is
//! [`crate::clipboard_pixels`].
//!
//! * **Copy** reads the active pixel layer inside the selection (the whole
//!   layer with none); **Copy Merged** the visible composite. Both fill the
//!   internal [`PixelClipboard`] (lossless `f16`) and put an 8-bit export on
//!   the system clipboard (an export in invariant 7.3.1b's sense).
//! * **Cut** is Copy plus erasing the same texels, one undo step "Cut"
//!   (refused on a pixels lock and on a transparency lock, like the Eraser).
//! * **Paste** makes a new pixel layer "Layer N" above the active node (the
//!   New Layer rule) holding the clip: one undo step "Paste". The layer is
//!   document-sized like a New Layer (grown only to hold a clip lying partly
//!   outside the canvas), so it can be painted on anywhere. The selection is
//!   left as it is. **Layer via Copy / Cut** are Copy / Cut plus Paste in
//!   Place as ONE step, without touching either clipboard.
//! * A focused text field keeps `Primary+C/X/V` (the field consumes them
//!   before the registry) and the other chords are not run from inside one
//!   (`handle_key`).

use aurora_core::Rect;

use crate::clipboard_pixels::{
    self as pixels, ClipError, ClipImage, OsPaste, PixelClip, paste_bounds, paste_origin,
};
use crate::fill_tools::Filled;
use crate::{
    App, ClipboardAccess, DabTool, LayerCommand, LayerCommandContext, LockRefusal, PaintPermission,
    UndoKind, UndoOrder, paint_permission, perform_layer_command,
};

/// One of the seven clipboard commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipCommand {
    Copy,
    Cut,
    CopyMerged,
    Paste,
    PasteInPlace,
    LayerViaCopy,
    LayerViaCut,
}

pub(crate) const COMMAND_COPY: &str = "edit.copy";
pub(crate) const COMMAND_CUT: &str = "edit.cut";
pub(crate) const COMMAND_COPY_MERGED: &str = "edit.copy_merged";
pub(crate) const COMMAND_PASTE: &str = "edit.paste";
pub(crate) const COMMAND_PASTE_IN_PLACE: &str = "edit.paste_in_place";
pub(crate) const COMMAND_LAYER_VIA_COPY: &str = "edit.layer_via_copy";
pub(crate) const COMMAND_LAYER_VIA_CUT: &str = "edit.layer_via_cut";

pub(crate) const CUT_LABEL: &str = "Cut";
pub(crate) const PASTE_LABEL: &str = "Paste";
pub(crate) const LAYER_VIA_COPY_LABEL: &str = "Layer via Copy";
pub(crate) const LAYER_VIA_CUT_LABEL: &str = "Layer via Cut";

/// Every command with its palette/menu title, in menu order.
pub(crate) const COMMANDS: [(&str, &str, ClipCommand); 7] = [
    (COMMAND_COPY, "Copy", ClipCommand::Copy),
    (COMMAND_CUT, "Cut", ClipCommand::Cut),
    (COMMAND_COPY_MERGED, "Copy Merged", ClipCommand::CopyMerged),
    (COMMAND_PASTE, "Paste", ClipCommand::Paste),
    (
        COMMAND_PASTE_IN_PLACE,
        "Paste in Place",
        ClipCommand::PasteInPlace,
    ),
    (
        COMMAND_LAYER_VIA_COPY,
        LAYER_VIA_COPY_LABEL,
        ClipCommand::LayerViaCopy,
    ),
    (
        COMMAND_LAYER_VIA_CUT,
        LAYER_VIA_CUT_LABEL,
        ClipCommand::LayerViaCut,
    ),
];

pub(crate) fn command_for(id: &str) -> Option<ClipCommand> {
    COMMANDS
        .iter()
        .find(|(command, _, _)| *command == id)
        .map(|(_, _, command)| *command)
}

impl ClipCommand {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn id(self) -> &'static str {
        COMMANDS
            .iter()
            .find(|(_, _, command)| *command == self)
            .map_or("", |(id, _, _)| id)
    }
}

/// The internal pixel clipboard: the lossless clip, and the fingerprint of
/// the last image Copy put on the system clipboard.
#[derive(Debug, Default)]
pub(crate) struct PixelClipboard {
    pub(crate) clip: Option<PixelClip>,
    pub(crate) exported: Option<u64>,
}

/// The active layer as a copy/cut source.
pub(crate) fn source_layer(
    layers: &aurora_doc::LayerTree,
    active: Option<aurora_doc::LayerId>,
) -> Result<(aurora_tile::SurfaceId, Rect), &'static str> {
    let Some(layer) = active else {
        return Err(pixels::NEEDS_PIXEL_LAYER_NOTICE);
    };
    let Some(aurora_doc::LayerKind::Pixel { bounds }) = layers.kind(layer).cloned() else {
        return Err(pixels::NEEDS_PIXEL_LAYER_NOTICE);
    };
    let Some(surface) = layers.surface_id(layer) else {
        return Err(pixels::NEEDS_PIXEL_LAYER_NOTICE);
    };
    Ok((surface, bounds))
}

/// Copy: the active layer's pixels under `selection`.
pub(crate) fn copy_layer(
    store: &mut aurora_tile::TileStore,
    layers: &aurora_doc::LayerTree,
    active: Option<aurora_doc::LayerId>,
    selection: Option<&aurora_doc::Selection>,
) -> Result<PixelClip, &'static str> {
    let (surface, bounds) = source_layer(layers, active)?;
    pixels::extract_layer(store, surface, bounds, selection).map_err(ClipError::notice)
}

/// Why a cut did not complete.
#[derive(Debug)]
pub(crate) enum CutError {
    /// Nothing was changed.
    Refused(&'static str),
    /// Some tiles were erased before a failure: record `filled` so the part
    /// that happened can be undone.
    Failed(&'static str, Filled),
}

/// Cut: Copy, then erase the copied texels through a stroke snapshot.
pub(crate) fn cut_active(
    store: &mut aurora_tile::TileStore,
    layers: &aurora_doc::LayerTree,
    active: Option<aurora_doc::LayerId>,
    selection: Option<&aurora_doc::Selection>,
) -> Result<(PixelClip, Filled), CutError> {
    let (surface, bounds) = source_layer(layers, active).map_err(CutError::Refused)?;
    let lock = active
        .and_then(|layer| layers.lock(layer))
        .unwrap_or_default();
    match paint_permission(DabTool::Eraser, lock) {
        PaintPermission::Refused(LockRefusal::PixelsLocked) => {
            return Err(CutError::Refused(pixels::CUT_PIXELS_LOCKED_NOTICE));
        }
        PaintPermission::Refused(_) => {
            return Err(CutError::Refused(pixels::CUT_TRANSPARENCY_LOCKED_NOTICE));
        }
        PaintPermission::Allowed | PaintPermission::PreserveAlpha => {}
    }
    let clip = pixels::extract_layer(store, surface, bounds, selection)
        .map_err(|err| CutError::Refused(err.notice()))?;
    let mut snapshot = aurora_brush::StrokeSnapshot::new(surface);
    match pixels::cut_layer(store, surface, bounds, selection, &mut snapshot) {
        Ok(painted) => Ok((clip, Filled { snapshot, painted })),
        Err(err) => {
            tracing::warn!(?err, "Cut could not erase part of the layer");
            Err(CutError::Failed(
                pixels::CUT_FAILED_NOTICE,
                Filled {
                    snapshot,
                    painted: Vec::new(),
                },
            ))
        }
    }
}

/// Fills the internal clipboard with `clip` and puts its 8-bit export on
/// the system clipboard (silently nothing if there is none).
pub(crate) fn publish(
    state: &mut PixelClipboard,
    clip: PixelClip,
    os: &mut dyn ClipboardAccess,
) {
    let rgba = clip.to_rgba8();
    state.exported = Some(pixels::fingerprint(clip.width(), clip.height(), &rgba));
    os.set_image(&ClipImage {
        width: clip.width(),
        height: clip.height(),
        rgba,
    });
    state.clip = Some(clip);
}

/// What a Paste pastes: the system clipboard's image when it is not the one
/// Copy exported, else the internal clip.
pub(crate) fn resolve_paste(
    state: &PixelClipboard,
    os: &mut dyn ClipboardAccess,
) -> Result<PixelClip, &'static str> {
    match pixels::os_image_paste(os.get_image(), state.exported) {
        OsPaste::Image(clip) => Ok(clip),
        OsPaste::TooLarge => Err(pixels::TOO_LARGE_TO_PASTE_NOTICE),
        OsPaste::Internal => state
            .clip
            .clone()
            .ok_or(pixels::NOTHING_TO_PASTE_NOTICE),
    }
}

/// The document rectangle a view of `size` logical pixels shows.
pub(crate) fn visible_rect(view: &aurora_ui::CanvasView, size: (f32, f32)) -> Option<Rect> {
    let a = view.to_document((0.0, 0.0));
    let b = view.to_document(size);
    let all = [a.0, a.1, b.0, b.1];
    if all.iter().any(|v| !v.is_finite() || v.abs() > 1.0e9) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    let (x0, y0, x1, y1) = (
        a.0.min(b.0).floor() as i64,
        a.1.min(b.1).floor() as i64,
        a.0.max(b.0).ceil() as i64,
        a.1.max(b.1).ceil() as i64,
    );
    Some(Rect {
        x: x0,
        y: y0,
        width: u32::try_from(x1 - x0).ok()?,
        height: u32::try_from(y1 - y0).ok()?,
    })
}

/// The document half of a paste, whole: places `clip` ([`paste_origin`]),
/// makes the new layer through [`crate::perform_layer_command`] (ends live
/// gestures, ONE structural step labelled `label`, rows rebuilt, the new
/// layer active), writes the pixels, and, for a Layer via Cut (`cut`), joins
/// the erasure to that step ([`UndoOrder::fuse_layer_via_cut`]) or takes it
/// back if the layer could not be made.
///
/// # Errors
///
/// The calm notice when the layer could not be made or filled.
pub(crate) fn place_clip_core(
    cx: &mut LayerCommandContext<'_>,
    store: &mut aurora_tile::TileStore,
    clip: PixelClip,
    visible: Option<Rect>,
    in_place: bool,
    label: &'static str,
    cut: Option<Filled>,
) -> Result<(), &'static str> {
    let canvas = cx.canvas_size;
    let clip = clip.moved_to(paste_origin(clip.rect(), visible, canvas, in_place));
    let too_big = clip.width() > aurora_core::MAX_DOCUMENT_EXTENT
        || clip.height() > aurora_core::MAX_DOCUMENT_EXTENT;
    let bounds = paste_bounds(canvas, clip.rect());
    let applied = !too_big && perform_layer_command(cx, LayerCommand::Paste { bounds, label });
    if !applied {
        if let Some(filled) = cut
            && let Err(err) = filled.snapshot.apply(store)
        {
            tracing::warn!(?err, "could not take back a cut");
        }
        cx.composite_cache.bump();
        return Err(pixels::PASTE_FAILED_NOTICE);
    }
    let surface = (*cx.active_layer).and_then(|layer| cx.layers.surface_id(layer));
    let written = surface.is_some_and(|surface| {
        pixels::write_clip(store, surface, bounds, &clip)
            .inspect_err(|err| tracing::warn!(?err, "a pasted layer could not be filled"))
            .is_ok()
    });
    if let Some(filled) = cut {
        if cx.pixel_history.push(filled.snapshot) {
            cx.undo_order.fuse_layer_via_cut();
        }
        crate::refresh_history_panel(cx.workspace, cx.undo_order);
    }
    cx.composite_cache.bump();
    if written {
        Ok(())
    } else {
        Err(pixels::PASTE_FAILED_NOTICE)
    }
}

impl UndoOrder {
    /// Turns the newest step (the structural "add layer" a Layer via Cut
    /// just recorded) into the combined step that also undoes the cut.
    pub(crate) fn fuse_layer_via_cut(&mut self) -> bool {
        self.sync_labels();
        match self.undo.last_mut() {
            Some(kind @ UndoKind::Structural) => {
                *kind = UndoKind::LayerViaCut;
                if let Some(label) = self.undo_labels.last_mut() {
                    LAYER_VIA_CUT_LABEL.clone_into(label);
                }
                true
            }
            _ => false,
        }
    }
}

impl App {
    fn clip_notice(&mut self, text: &'static str) {
        self.lock_notice = Some((LockRefusal::Path(text), self.doc.active_layer));
        self.sync_status_bar_now();
        self.needs_redraw = true;
    }

    /// Runs one clipboard command.
    pub(crate) fn run_clipboard_command(&mut self, command: ClipCommand) {
        if self.quick_mask_refuses_command() {
            return;
        }
        crate::lasso_tools::cancel_selection_gesture(&mut self.drag, &mut self.doc.selection);
        match command {
            ClipCommand::Copy => self.copy_command(false),
            ClipCommand::CopyMerged => self.copy_command(true),
            ClipCommand::Cut | ClipCommand::LayerViaCut => self.cut_command(command),
            ClipCommand::Paste | ClipCommand::PasteInPlace => {
                let clip = resolve_paste(&self.pixel_clip, &mut self.clipboard);
                match clip {
                    Ok(clip) => {
                        let label = PASTE_LABEL;
                        self.place_clip(clip, command == ClipCommand::PasteInPlace, label, None);
                    }
                    Err(text) => self.clip_notice(text),
                }
            }
            ClipCommand::LayerViaCopy => {
                let selection = self.doc.selection.active();
                let Some(store) = self.doc.tile_store.as_mut() else {
                    return;
                };
                match copy_layer(
                    store,
                    &self.doc.layers,
                    self.doc.active_layer,
                    selection.as_ref(),
                ) {
                    Ok(clip) => {
                        self.place_clip(clip, true, LAYER_VIA_COPY_LABEL, None);
                    }
                    Err(text) => self.clip_notice(text),
                }
            }
        }
    }

    fn copy_command(&mut self, merged: bool) {
        let selection = self.doc.selection.active();
        let canvas = self.doc.canvas_size;
        let Some(store) = self.doc.tile_store.as_mut() else {
            return;
        };
        let result = if merged {
            pixels::extract_merged(&self.doc.layers, store, canvas, selection.as_ref())
                .map_err(ClipError::notice)
        } else {
            copy_layer(
                store,
                &self.doc.layers,
                self.doc.active_layer,
                selection.as_ref(),
            )
        };
        match result {
            Ok(clip) => publish(&mut self.pixel_clip, clip, &mut self.clipboard),
            Err(text) => self.clip_notice(text),
        }
    }

    fn cut_command(&mut self, command: ClipCommand) {
        let canvas_area = crate::canvas_area_logical_size(&self.workspace);
        let _ = crate::commit_drag_into_history(
            &mut self.workspace,
            self.drag.take(),
            &self.doc.layers,
            &mut self.doc.history,
            &mut self.doc.pixel_history,
            &mut self.doc.undo_order,
            &mut self.doc.canvas_view,
            self.doc.active_layer,
            canvas_area,
        );
        let selection = self.doc.selection.active();
        let layer = self.doc.active_layer;
        let Some(store) = self.doc.tile_store.as_mut() else {
            return;
        };
        let outcome = cut_active(store, &self.doc.layers, layer, selection.as_ref());
        let via_layer = command == ClipCommand::LayerViaCut;
        match outcome {
            Err(CutError::Refused(text)) => self.clip_notice(text),
            Err(CutError::Failed(text, filled)) => {
                if let Some(layer) = layer {
                    self.finish_fill(layer, Ok(filled), CUT_LABEL);
                }
                self.clip_notice(text);
            }
            Ok((clip, filled)) => {
                if via_layer {
                    self.place_clip(clip, true, LAYER_VIA_CUT_LABEL, Some(filled));
                } else {
                    publish(&mut self.pixel_clip, clip, &mut self.clipboard);
                    if let Some(layer) = layer {
                        self.finish_fill(layer, Ok(filled), CUT_LABEL);
                    }
                }
            }
        }
    }

    /// Puts `clip` in a new layer as one step labelled `label` (see
    /// [`place_clip_core`]); a failure becomes the calm notice.
    fn place_clip(
        &mut self,
        clip: PixelClip,
        in_place: bool,
        label: &'static str,
        cut: Option<Filled>,
    ) {
        let visible = crate::canvas_area_logical_size(&self.workspace)
            .and_then(|size| visible_rect(&self.doc.canvas_view, size));
        let Some(store) = self.doc.tile_store.as_mut() else {
            return;
        };
        let result = place_clip_core(
            &mut LayerCommandContext {
                workspace: &mut self.workspace,
                focus: &mut self.focus,
                scales: &self.scales,
                layers: &mut self.doc.layers,
                history: &mut self.doc.history,
                pixel_history: &mut self.doc.pixel_history,
                undo_order: &mut self.doc.undo_order,
                layer_rows: &mut self.layer_rows,
                active_layer: &mut self.doc.active_layer,
                view: &mut self.doc.canvas_view,
                composite_cache: &mut self.doc.composite_cache,
                drag: &mut self.drag,
                layer_controls: &mut self.layer_controls,
                click: &mut self.gallery_click,
                canvas_size: self.doc.canvas_size,
            },
            store,
            clip,
            visible,
            in_place,
            label,
            cut,
        );
        let window_size = self.window.as_ref().map(|window| window.inner_size());
        if let Some(size) = window_size {
            self.apply_resize((size.width, size.height));
        }
        self.push_accessibility();
        self.needs_redraw = true;
        if let Err(text) = result {
            self.clip_notice(text);
        }
    }
}
