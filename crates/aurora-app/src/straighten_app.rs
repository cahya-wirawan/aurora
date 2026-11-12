//! `App`'s half of Straighten (task 180): the "Crop: Straighten by Angle..."
//! prompt, the Straighten line the options bar's checkbox turns on, the
//! overlay of the rotated frame and the commit. The geometry and the
//! resampler are [`crate::straighten`], the document operation
//! [`crate::straighten_doc`] (pure, tested headlessly); this file is the
//! glue that needs `App`'s fields and is covered by those tests and by
//! reading.
//!
//! **The prompt** is the command palette's prompt widget, like Select >
//! Colour Range's: one row saying what Enter does ("Straighten by -1.5
//! degrees"), the typed text starting as the angle in force (or `0`), a
//! number from -45 to 45 (fractions allowed); `0` clears the angle. An
//! invalid one keeps the prompt open and shows the reason. The command is
//! for the Crop tool only: with another tool active it says so in the
//! status bar and does nothing (the palette does not grey commands out).

use aurora_core::Rect;
use aurora_doc::{Anchor, Path, Subpath};
use aurora_widgets::widgets::{
    CommandEntry, OutlineRun, command_palette_state, insert_command_prompt,
    set_command_palette_commands, set_command_palette_message, set_command_palette_query,
};

use crate::crop::CropRefusal;
use crate::straighten::{self, MIN_LINE_LOGICAL_PX, checked_angle};
use crate::{
    App, FocusManager, LockRefusal, WidgetId, command_palette_style, guarded_scale_factor,
};

/// The prompt's accessible name.
const PROMPT_LABEL: &str = "Straighten Angle in Degrees";

/// The id of the prompt's one row.
const ROW_ID: &str = "crop.straighten_by_angle.apply";

/// The angle typed into the prompt, or the reason it is refused.
///
/// # Errors
///
/// A short sentence for the prompt's row.
pub(crate) fn parse_angle(text: &str) -> Result<f64, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Enter an angle from -45 to 45 degrees.".to_owned());
    }
    let numeric = text
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'));
    if !numeric {
        return Err("Use a number of degrees, for example -1.5.".to_owned());
    }
    let Ok(value) = text.parse::<f64>() else {
        return Err("That is not a number.".to_owned());
    };
    checked_angle(value)?;
    Ok(value)
}

/// The text the prompt starts with for `angle`.
fn start_text(angle: Option<f64>) -> String {
    angle.map_or_else(|| "0".to_owned(), |a| format!("{a:.2}"))
}

/// The prompt's one row for the typed `query`: what Enter would do.
fn prompt_entries(query: &str) -> Vec<CommandEntry> {
    let amount = query.trim();
    let title = if amount.is_empty() {
        "Straighten".to_owned()
    } else {
        format!("Straighten by {amount} degrees")
    };
    vec![CommandEntry::new(ROW_ID, title)]
}

/// Opens the prompt (a no-op returning `false` if a palette or prompt is
/// open), the typed text starting as `start`.
pub(crate) fn open_prompt(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
    start: &str,
) -> bool {
    if palette.is_some() {
        return false;
    }
    let root = match insert_command_prompt(
        &mut workspace.tree,
        workspace.root,
        PROMPT_LABEL,
        prompt_entries(start),
        &workspace.scales,
    ) {
        Ok(root) => root,
        Err(err) => {
            tracing::warn!(?err, "failed to open the Straighten prompt");
            return false;
        }
    };
    if let Err(err) = workspace.tree.set_style(root, command_palette_style()) {
        tracing::warn!(?err, "failed to size the Straighten prompt");
    }
    if let Err(err) = set_command_palette_query(&mut workspace.tree, root, start) {
        tracing::warn!(?err, "failed to fill the Straighten prompt");
    }
    if let Err(err) = focus.focus(&mut workspace.tree, root) {
        tracing::warn!(?err, "failed to focus the Straighten prompt");
    }
    *palette = Some(root);
    true
}

/// Whether `root` is the Straighten prompt.
fn is_prompt(tree: &aurora_widgets::WidgetTree<crate::WidgetKind>, root: WidgetId) -> bool {
    let Ok(state) = command_palette_state(tree, root) else {
        return false;
    };
    !state.is_filtering() && state.commands().first().is_some_and(|c| c.id == ROW_ID)
}

/// After the typed text changed: the row follows it and a refusal is
/// withdrawn. Returns whether `root` is the Straighten prompt.
pub(crate) fn follow_prompt_query(
    tree: &mut aurora_widgets::WidgetTree<crate::WidgetKind>,
    root: WidgetId,
) -> bool {
    if !is_prompt(tree, root) {
        return false;
    }
    let Ok(state) = command_palette_state(tree, root) else {
        return true;
    };
    let query = state.query().to_owned();
    if let Err(err) = set_command_palette_commands(tree, root, prompt_entries(&query)) {
        tracing::warn!(?err, "failed to update the Straighten prompt");
    }
    if let Err(err) = set_command_palette_message(tree, root, None) {
        tracing::warn!(?err, "failed to clear the Straighten prompt's message");
    }
    true
}

/// The prompt's Enter. `None` when `root` is not the Straighten prompt;
/// `Some(Ok(degrees))` for a valid angle (the caller closes the prompt and
/// applies it); `Some(Err(()))` for a refused one: the prompt stays open and
/// shows the reason.
pub(crate) fn prompt_enter(
    tree: &mut aurora_widgets::WidgetTree<crate::WidgetKind>,
    root: WidgetId,
) -> Option<Result<f64, ()>> {
    if !is_prompt(tree, root) {
        return None;
    }
    let query = command_palette_state(tree, root).ok()?.query().to_owned();
    match parse_angle(&query) {
        Ok(degrees) => Some(Ok(degrees)),
        Err(message) => {
            let row = vec![CommandEntry::new(ROW_ID, message.clone())];
            if let Err(err) = set_command_palette_commands(tree, root, row) {
                tracing::warn!(?err, "failed to show a refused angle");
            }
            if let Err(err) = set_command_palette_message(tree, root, Some(message)) {
                tracing::warn!(?err, "failed to describe a refused angle");
            }
            Some(Err(()))
        }
    }
}

/// The overlay path of a rotated crop frame: the eight handle points around
/// the perimeter as a closed subpath of corner anchors.
pub(crate) fn frame_path(frame: &straighten::Frame, rect: &Rect) -> Path {
    let anchors = frame
        .handle_points(rect)
        .iter()
        .map(|&(x, y)| {
            #[allow(clippy::cast_possible_truncation)]
            Anchor::corner(x as f32, y as f32)
        })
        .collect();
    Path {
        name: "Straighten frame".to_owned(),
        subpaths: vec![Subpath {
            anchors,
            closed: true,
        }],
    }
}

/// The overlay path of the Straighten line being drawn.
pub(crate) fn line_path(from: (f32, f32), to: (f32, f32)) -> Path {
    Path {
        name: "Straighten line".to_owned(),
        subpaths: vec![Subpath {
            anchors: vec![Anchor::corner(from.0, from.1), Anchor::corner(to.0, to.1)],
            closed: false,
        }],
    }
}

impl App {
    /// Whether the Straighten checkbox is on and the Crop tool is active:
    /// a drag then draws the Straighten line.
    pub(crate) fn straighten_line_armed(&self) -> bool {
        self.tool == aurora_ui::Tool::Crop && self.tool_settings.crop_straighten
    }

    /// "Crop: Straighten by Angle...": opens the prompt, or says why not.
    pub(crate) fn open_straighten_prompt(&mut self) {
        if self.quick_mask_refuses_command() {
            return;
        }
        if self.tool != aurora_ui::Tool::Crop {
            self.straighten_notice(LockRefusal::StraightenNeedsCropTool);
            return;
        }
        let start = start_text(self.crop.angle());
        if open_prompt(
            &mut self.workspace,
            &mut self.focus,
            &mut self.command_palette,
            &start,
        ) {
            self.push_accessibility();
            self.needs_redraw = true;
        }
    }

    /// Sets the crop angle from the prompt or a typed call: `0` clears it.
    pub(crate) fn run_straighten_angle(&mut self, degrees: f64) {
        if self.quick_mask_refuses_command() {
            return;
        }
        if self.tool != aurora_ui::Tool::Crop {
            self.straighten_notice(LockRefusal::StraightenNeedsCropTool);
            return;
        }
        self.apply_straighten_angle(degrees);
    }

    /// Sets (or, for `0`, clears) the crop angle; a canvas too small for any
    /// rectangle at the angle is refused with the degenerate-crop notice.
    fn apply_straighten_angle(&mut self, degrees: f64) {
        let Ok(angle) = checked_angle(degrees) else {
            self.straighten_notice(LockRefusal::CropDegenerate);
            return;
        };
        self.sync_crop_ratio_now();
        let changed = self
            .crop
            .set_angle(angle, self.doc.id, self.doc.canvas_size);
        if angle.is_some() && !changed {
            self.straighten_notice(LockRefusal::CropDegenerate);
            return;
        }
        self.lock_notice = None;
        self.crop_changed_visibly();
    }

    fn straighten_notice(&mut self, why: LockRefusal) {
        self.lock_notice = Some((why, self.doc.active_layer));
        self.sync_status_bar_now();
        self.needs_redraw = true;
    }

    /// The Straighten line was released: a long enough one sets the angle and
    /// turns the checkbox off, a shorter one changes nothing. Called with the
    /// line the drag drew.
    pub(crate) fn straighten_line_released(&mut self, line: ((f32, f32), (f32, f32))) {
        let zoom = self.doc.canvas_view.zoom();
        let min_len = if zoom.is_finite() && zoom > 0.0 {
            MIN_LINE_LOGICAL_PX / zoom
        } else {
            0.0
        };
        let Some(degrees) = straighten::angle_from_line(line.0, line.1, min_len) else {
            return;
        };
        self.apply_straighten_angle(degrees);
        // Photoshop's Straighten ends with the line: the box turns itself off.
        self.tool_settings.crop_straighten = false;
        self.sync_tool_controls_now();
        self.push_accessibility();
    }

    /// The overlay of the rotated frame and of the Straighten line being
    /// drawn, for this frame (empty without either).
    pub(crate) fn straighten_overlay_runs_now(&self) -> Vec<OutlineRun> {
        if self.tool != aurora_ui::Tool::Crop {
            return Vec::new();
        }
        let Some(size) = crate::canvas_area_logical_size(&self.workspace) else {
            return Vec::new();
        };
        let scale = guarded_scale_factor(self.scale_factor);
        let mut runs = Vec::new();
        if let (Some(frame), Some(rect)) = (self.crop.frame(self.doc.canvas_size), self.crop.rect())
        {
            runs.extend(aurora_ui::path_overlay_runs(
                &frame_path(&frame, &rect),
                None,
                &self.doc.canvas_view,
                size,
                scale,
                self.crop_handle_px(),
            ));
        }
        if let Some((from, to)) = self.crop.line() {
            runs.extend(aurora_ui::path_overlay_runs(
                &line_path(from, to),
                None,
                &self.doc.canvas_view,
                size,
                scale,
                0,
            ));
        }
        runs
    }

    /// Commits the straightened crop. All or nothing; says why not.
    pub(crate) fn straighten_commit(&mut self, angle: f64, rect: Rect) {
        let Some(store) = self.doc.tile_store.as_mut() else {
            self.crop_refused(CropRefusal::Failed);
            return;
        };
        let started = std::time::Instant::now();
        let result = crate::straighten_doc::straighten_document(
            &mut self.doc.layers,
            &mut self.doc.history,
            &mut self.doc.pixel_history,
            &mut self.doc.undo_order,
            &mut self.doc.selection,
            &mut self.doc.canvas_size,
            store,
            self.quick_mask.is_some(),
            angle,
            rect,
        );
        tracing::info!(elapsed = ?started.elapsed(), ok = result.is_ok(), "straighten");
        match result {
            Ok(()) => {
                self.lock_notice = None;
                crate::refresh_history_panel(&mut self.workspace, &self.doc.undo_order);
                self.after_canvas_changed();
            }
            Err(why) => self.crop_refused(why),
        }
    }

    /// Applies what an undo, redo or History jump of a straighten left
    /// pending (the selections, paths and canvas size); returns whether there
    /// was anything.
    pub(crate) fn apply_pending_straighten_now(&mut self) -> bool {
        if crate::straighten_doc::apply_pending_straighten(
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
}
