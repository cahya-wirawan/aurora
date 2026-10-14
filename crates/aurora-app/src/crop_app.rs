//! `App`'s half of the Crop tool (0.226.0): the pointer, keys, commit,
//! "Crop to Selection", the overlay and the view reset after the document
//! changed size. The geometry and the document operation are
//! [`crate::crop`] (pure, tested headlessly); this file is only the glue
//! that needs `App`'s fields, and no window is built in tests, so it is
//! covered by those tests and by reading.

use std::time::Instant;

use aurora_widgets::shortcut::NamedKey;

use crate::crop::{self, CropRefusal};
use crate::{App, Key, guarded_scale_factor};

impl App {
    /// The crop handle's side in physical pixels (`spacing.xs` logical).
    fn crop_handle_px(&self) -> u32 {
        let side =
            f64::from(self.scales.spacing.xs) * f64::from(guarded_scale_factor(self.scale_factor));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let px = side.round().max(1.0) as u32;
        px
    }

    /// How far from a handle a press still grabs it, in document pixels
    /// (`spacing.sm` logical screen pixels at the current zoom).
    fn crop_hit_radius(&self) -> f32 {
        let zoom = self.doc.canvas_view.zoom();
        if zoom.is_finite() && zoom > 0.0 {
            f32::from(u16::try_from(self.scales.spacing.sm).unwrap_or(12)) / zoom
        } else {
            0.0
        }
    }

    /// A primary press on the canvas with the Crop tool.
    pub(crate) fn crop_press(&mut self, canvas_point: (f32, f32)) {
        let doc_point = self.doc.canvas_view.to_document(canvas_point);
        let radius = self.crop_hit_radius();
        match self.crop.press(
            doc_point,
            radius,
            Instant::now(),
            self.doc.id,
            self.doc.canvas_size,
        ) {
            crop::Press::Started => {}
            crop::Press::Commit(rect) => self.crop_commit_rect(rect),
        }
        self.crop_changed_visibly();
    }

    /// The pointer moved with a crop drag live; returns whether it did.
    pub(crate) fn crop_pointer_moved(&mut self, canvas_point: (f32, f32)) -> bool {
        if !self.crop.dragging() {
            return false;
        }
        let doc_point = self.doc.canvas_view.to_document(canvas_point);
        self.crop.drag(doc_point, self.doc.canvas_size);
        self.crop_changed_visibly();
        true
    }

    /// The primary button was released: a crop drag ends, the rectangle stays.
    pub(crate) fn crop_pointer_released(&mut self) {
        if self.crop.dragging() {
            self.crop.release();
            self.crop_changed_visibly();
        }
    }

    fn crop_changed_visibly(&mut self) {
        self.needs_redraw = true;
        self.sync_status_bar_now();
    }

    /// Enter commits and Escape cancels a crop rectangle, and (0.231.0) the
    /// arrow keys nudge it (see [`crop::nudge_for_key`]), offered after the
    /// focused widgets. Returns whether the key was used.
    pub(crate) fn crop_key_now(&mut self, key: Key) -> bool {
        let action = crop_key_action(
            self.tool,
            self.crop.rect().is_some(),
            self.dialog.is_some() || self.command_palette.is_some(),
            key,
            self.modifiers,
        );
        match action {
            CropKeyAction::Ignore => return false,
            CropKeyAction::Commit => {
                if let Some(rect) = self.crop.rect() {
                    self.crop_commit_rect(rect);
                }
            }
            CropKeyAction::Cancel => self.crop.cancel(),
            // Used even while a drag is live (then it changes nothing), so
            // an arrow never also reaches a shortcut.
            CropKeyAction::Nudge(nudge) => {
                self.crop.nudge(nudge, self.doc.canvas_size);
            }
        }
        self.crop_changed_visibly();
        true
    }

    /// The cursor the Crop tool wants right now: its pointer is over the
    /// canvas, no dialog or palette is open.
    fn wanted_crop_cursor(&self) -> crop::CropCursor {
        let over_canvas = if self.dialog.is_some() || self.command_palette.is_some() {
            None
        } else {
            self.pointer_position
                .and_then(|position| crate::pointer_in_canvas(&self.workspace, position))
        };
        let pointer = over_canvas.map(|point| self.doc.canvas_view.to_document(point));
        let rect = self.crop.rect();
        crop::cursor_for(self.tool, rect.as_ref(), pointer, self.crop_hit_radius())
    }

    /// Hands the window the Crop tool's cursor when it differs from the one
    /// last set (run once per loop turn). Every other tool keeps the
    /// window's ordinary pointer; the app sets no other cursor.
    pub(crate) fn sync_crop_cursor_now(&mut self) {
        let wanted = self.wanted_crop_cursor();
        if wanted == self.crop_cursor {
            return;
        }
        self.crop_cursor = wanted;
        if let Some(window) = self.window.as_ref() {
            window.set_cursor(wanted.icon());
        }
    }

    /// Cancels a crop rectangle that no longer belongs to the active tool,
    /// document or canvas size (run once per loop turn).
    pub(crate) fn crop_drop_if_stale(&mut self) {
        if self.crop.rect().is_none() && !self.crop.dragging() {
            return;
        }
        if self.tool != aurora_ui::Tool::Crop
            || self.crop.is_stale(self.doc.id, self.doc.canvas_size)
        {
            self.crop.cancel();
            self.crop_changed_visibly();
        }
    }

    /// Crops the document to `rect` as one "Crop" step, or says why not.
    pub(crate) fn crop_commit_rect(&mut self, rect: aurora_core::Rect) {
        // A live layer drag ends (and is recorded) before the crop.
        let ending = self.drag.take();
        self.commit_drag(ending);
        match crop::crop_document(
            &mut self.doc.layers,
            &mut self.doc.history,
            &mut self.doc.pixel_history,
            &mut self.doc.undo_order,
            &mut self.doc.selection,
            &mut self.doc.canvas_size,
            self.quick_mask.is_some(),
            rect,
        ) {
            Ok(()) => {
                self.lock_notice = None;
                crate::refresh_history_panel(&mut self.workspace, &self.doc.undo_order);
                self.after_canvas_changed();
            }
            Err(why) => self.crop_refused(why),
        }
    }

    fn crop_refused(&mut self, why: CropRefusal) {
        self.crop.cancel();
        self.lock_notice = Some((why.notice(), self.doc.active_layer));
        self.sync_status_bar_now();
        self.needs_redraw = true;
    }

    /// "Crop to Selection": the active selection's bounds, clipped to the
    /// canvas. Pixels outside a non-rectangular selection are kept.
    pub(crate) fn crop_to_selection(&mut self) {
        if self.quick_mask.is_some() {
            self.crop_refused(CropRefusal::QuickMask);
            return;
        }
        match crop::selection_crop_rect(self.doc.selection.active().as_ref(), self.doc.canvas_size)
        {
            Ok(rect) => self.crop_commit_rect(rect),
            Err(why) => self.crop_refused(why),
        }
    }

    /// Applies what an undo, redo or History jump of a crop left pending
    /// (the canvas size and the selection shift); returns whether there was
    /// anything.
    pub(crate) fn apply_pending_crop_now(&mut self) -> bool {
        if crop::apply_pending_crop(
            &mut self.doc.undo_order,
            &mut self.doc.canvas_size,
            &mut self.doc.selection,
        ) {
            self.after_canvas_changed();
            true
        } else {
            false
        }
    }

    /// The document's size or origin changed (a crop, its undo or redo): the
    /// rectangle goes, every composite tile and atlas slot is stale, and the
    /// view starts again at the new canvas (the same reset an Open does).
    fn after_canvas_changed(&mut self) {
        self.crop.cancel();
        self.doc.composite_cache.bump();
        if let Some(residency) = self.residency.as_mut() {
            residency.forget_slots();
        }
        self.doc.canvas_view = crate::load_document_view(
            &self.doc.canvas_view,
            &self.doc.layers,
            self.doc.active_layer,
            crate::canvas_area_physical_size(&self.workspace, self.scale_factor),
            crate::canvas_area_logical_size(&self.workspace),
            self.scale_factor,
        );
        self.needs_redraw = true;
        self.sync_status_bar_now();
    }

    /// What the selection outline widget draws: the crop rectangle while the
    /// Crop tool has one, else the active selection.
    pub(crate) fn outline_selection(&self) -> Option<aurora_doc::Selection> {
        match (self.tool, self.crop.rect()) {
            (aurora_ui::Tool::Crop, Some(rect)) => Some(aurora_doc::Selection::new(rect)),
            _ => self.doc.selection.active(),
        }
    }

    /// The handles of the crop rectangle for this frame (empty without one).
    pub(crate) fn crop_handle_runs_now(&self) -> Vec<aurora_widgets::widgets::OutlineRun> {
        let (aurora_ui::Tool::Crop, Some(rect), Some(size)) = (
            self.tool,
            self.crop.rect(),
            crate::canvas_area_logical_size(&self.workspace),
        ) else {
            return Vec::new();
        };
        aurora_ui::crop_handle_runs(
            &rect,
            &self.doc.canvas_view,
            size,
            guarded_scale_factor(self.scale_factor),
            self.crop_handle_px(),
        )
    }

    /// The rule-of-thirds guides of the crop rectangle for this frame
    /// (empty without one).
    pub(crate) fn crop_guide_runs_now(&self) -> Vec<aurora_widgets::widgets::OutlineRun> {
        guides_for_frame(
            self.tool,
            self.crop.rect(),
            crate::canvas_area_logical_size(&self.workspace),
            &self.doc.canvas_view,
            guarded_scale_factor(self.scale_factor),
        )
    }

    /// The Crop shield for this frame: the canvas area outside the crop
    /// rectangle while the Crop tool has one (empty otherwise).
    pub(crate) fn crop_shield_rects_now(&self) -> Vec<aurora_widgets::widgets::ShieldRect> {
        shield_for_frame(
            self.tool,
            self.crop.rect(),
            crate::canvas_area_logical_size(&self.workspace),
            &self.doc.canvas_view,
            guarded_scale_factor(self.scale_factor),
        )
    }

    /// Draws this frame's Crop shield on the workspace's outline widget.
    pub(crate) fn sync_crop_shield_now(&mut self) {
        let rects = self.crop_shield_rects_now();
        if let Err(err) = aurora_ui::sync_crop_shield(
            &mut self.workspace.tree,
            self.workspace.selection_outline,
            rects,
        ) {
            tracing::warn!(?err, "failed to update the crop shield");
        }
    }
}

/// [`App::crop_shield_rects_now`]'s decision as a free function: shield
/// only while the Crop tool is active, has a rectangle and the canvas area
/// has a size.
pub(crate) fn shield_for_frame(
    tool: aurora_ui::Tool,
    rect: Option<aurora_core::Rect>,
    canvas_size: Option<(f32, f32)>,
    view: &aurora_ui::CanvasView,
    scale_factor: f32,
) -> Vec<aurora_widgets::widgets::ShieldRect> {
    let (aurora_ui::Tool::Crop, Some(rect), Some(size)) = (tool, rect, canvas_size) else {
        return Vec::new();
    };
    aurora_ui::crop_shield_rects(&rect, view, size, scale_factor)
}

/// [`App::crop_guide_runs_now`]'s decision as a free function: guides only
/// while the Crop tool is active, has a rectangle and the canvas area has a
/// size.
pub(crate) fn guides_for_frame(
    tool: aurora_ui::Tool,
    rect: Option<aurora_core::Rect>,
    canvas_size: Option<(f32, f32)>,
    view: &aurora_ui::CanvasView,
    scale_factor: f32,
) -> Vec<aurora_widgets::widgets::OutlineRun> {
    let (aurora_ui::Tool::Crop, Some(rect), Some(size)) = (tool, rect, canvas_size) else {
        return Vec::new();
    };
    aurora_ui::crop_guide_runs(&rect, view, size, scale_factor)
}

/// What a key means to the Crop tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CropKeyAction {
    /// Not the Crop tool's key (or no rectangle, or a modal is open): the
    /// key goes on to the shortcuts.
    Ignore,
    Commit,
    Cancel,
    Nudge(crop::Nudge),
}

/// [`App::crop_key_now`]'s decision as a free function: the keys are only
/// the Crop tool's while it is active and holds a rectangle and no dialog
/// or palette is open.
pub(crate) fn crop_key_action(
    tool: aurora_ui::Tool,
    has_rect: bool,
    modal_open: bool,
    key: Key,
    modifiers: aurora_widgets::shortcut::Modifiers,
) -> CropKeyAction {
    if tool != aurora_ui::Tool::Crop || !has_rect || modal_open {
        return CropKeyAction::Ignore;
    }
    match key {
        Key::Named(NamedKey::Enter) => CropKeyAction::Commit,
        Key::Named(NamedKey::Escape) => CropKeyAction::Cancel,
        _ => {
            crop::nudge_for_key(key, modifiers).map_or(CropKeyAction::Ignore, CropKeyAction::Nudge)
        }
    }
}
