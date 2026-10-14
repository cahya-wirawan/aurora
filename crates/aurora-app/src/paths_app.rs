//! `App`'s half of the Paths feature (0.233.0): the Pen tool's pointer and
//! keys, the Paths panel's rows and toolbar, the commands (Make Selection
//! from Path, New, Delete, Rename, Save Work Path) and the overlay. The
//! state machine is [`crate::pen`] and the model is `aurora_doc::PathSet`;
//! what is here is glue plus the pure decisions the glue makes, which the
//! tests drive without a window.

use std::time::Instant;

use aurora_doc::{PathSet, Selection, WORK_PATH_NAME};
use aurora_widgets::shortcut::NamedKey;
use aurora_widgets::widgets::{
    CommandEntry, command_palette_state, insert_command_prompt, set_command_palette_commands,
    set_command_palette_message, set_command_palette_query,
};

use crate::document_session::DocumentSession;
use crate::pen::{self, PenOutcome};
use crate::{App, FocusManager, Key, LockRefusal, WidgetId, command_palette_style};

pub(crate) const COMMAND_MAKE_SELECTION: &str = "paths.make_selection";
pub(crate) const MAKE_SELECTION_TITLE: &str = "Make Selection from Path";
pub(crate) const COMMAND_SAVE_WORK: &str = "paths.save_work";
pub(crate) const SAVE_WORK_TITLE: &str = "Save Work Path\u{2026}";
pub(crate) const COMMAND_NEW: &str = "paths.new";
pub(crate) const NEW_TITLE: &str = "New Path";
pub(crate) const COMMAND_DELETE: &str = "paths.delete";
pub(crate) const DELETE_TITLE: &str = "Delete Path";
pub(crate) const COMMAND_RENAME: &str = "paths.rename";
pub(crate) const RENAME_TITLE: &str = "Rename Path\u{2026}";

/// The palette entries of the Paths commands (not the Pen's tool entry,
/// which every tool has).
pub(crate) const COMMANDS: [(&str, &str); 5] = [
    (COMMAND_MAKE_SELECTION, MAKE_SELECTION_TITLE),
    (COMMAND_NEW, NEW_TITLE),
    (COMMAND_DELETE, DELETE_TITLE),
    (COMMAND_RENAME, RENAME_TITLE),
    (COMMAND_SAVE_WORK, SAVE_WORK_TITLE),
];

/// The prompt's accessible name and its one row.
const PROMPT_LABEL: &str = "Path Name";
const ROW_NAME: &str = "paths.name.apply";

pub(crate) const NEW_PATH_LABEL: &str = "New Path";
pub(crate) const DELETE_PATH_LABEL: &str = "Delete Path";
pub(crate) const RENAME_PATH_LABEL: &str = "Rename Path";
pub(crate) const SAVE_WORK_LABEL: &str = "Save Work Path";

pub(crate) const PATHS_LOST_NOTICE: &str =
    "The crop moved this document's paths out of range, so they were dropped.";
pub(crate) const NO_PATH_NOTICE: &str = "Select a path in the Paths panel first.";
pub(crate) const EMPTY_PATH_NOTICE: &str = "This path has no area to select.";
pub(crate) const NO_WORK_PATH_NOTICE: &str = "There is no work path to save.";
pub(crate) const NAME_TAKEN_NOTICE: &str = "That path name can't be used here; it may be taken.";
pub(crate) const PATHS_FULL_NOTICE: &str = "This document already has the most paths it can hold.";
pub(crate) const SELECTION_FAILED_NOTICE: &str = "That path is too large to make a selection from.";

/// A Paths command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathsCommand {
    MakeSelection,
    SaveWork,
    New,
    Delete,
    Rename,
    /// The name the prompt accepted (for the path the prompt was opened on).
    Name(String),
}

/// The command a palette id stands for.
pub(crate) fn command_for(id: &str) -> Option<PathsCommand> {
    match id {
        COMMAND_MAKE_SELECTION => Some(PathsCommand::MakeSelection),
        COMMAND_SAVE_WORK => Some(PathsCommand::SaveWork),
        COMMAND_NEW => Some(PathsCommand::New),
        COMMAND_DELETE => Some(PathsCommand::Delete),
        COMMAND_RENAME => Some(PathsCommand::Rename),
        _ => None,
    }
}

// ---------------------------------------------------------------- prompt

fn prompt_entries(query: &str) -> Vec<CommandEntry> {
    let name = query.trim();
    let title = if name.is_empty() {
        "Name Path".to_owned()
    } else {
        format!("Name Path \u{201c}{name}\u{201d}")
    };
    vec![CommandEntry::new(ROW_NAME, title)]
}

/// Opens the name prompt (`false` if a palette or prompt is open).
pub(crate) fn open_prompt(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
) -> bool {
    if palette.is_some() {
        return false;
    }
    let root = match insert_command_prompt(
        &mut workspace.tree,
        workspace.root,
        PROMPT_LABEL,
        prompt_entries(""),
    ) {
        Ok(root) => root,
        Err(err) => {
            tracing::warn!(?err, "failed to open the path name prompt");
            return false;
        }
    };
    if let Err(err) = workspace.tree.set_style(root, command_palette_style()) {
        tracing::warn!(?err, "failed to size the path name prompt");
    }
    if let Err(err) = set_command_palette_query(&mut workspace.tree, root, "") {
        tracing::warn!(?err, "failed to clear the path name prompt");
    }
    if let Err(err) = focus.focus(&mut workspace.tree, root) {
        tracing::warn!(?err, "failed to focus the path name prompt");
    }
    *palette = Some(root);
    true
}

fn is_prompt(tree: &aurora_widgets::WidgetTree<crate::WidgetKind>, root: WidgetId) -> bool {
    command_palette_state(tree, root).is_ok_and(|state| {
        !state.is_filtering() && state.commands().first().is_some_and(|c| c.id == ROW_NAME)
    })
}

/// After the typed text changed: the row follows it and a refusal is
/// withdrawn. Returns whether `root` is this prompt.
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
        tracing::warn!(?err, "failed to update the path name prompt");
    }
    if let Err(err) = set_command_palette_message(tree, root, None) {
        tracing::warn!(?err, "failed to clear the path name prompt's message");
    }
    true
}

/// A refused name's sentence.
pub(crate) const fn refusal_message() -> &'static str {
    "Type a name of up to 64 characters, without control characters; \u{201c}Work Path\u{201d} is reserved."
}

/// The prompt's Enter. `None` when `root` is not this prompt;
/// `Some(Ok(name))` for a valid name; `Some(Err(()))` for a refused one,
/// which stays open with the reason in its row and description.
pub(crate) fn prompt_enter(
    tree: &mut aurora_widgets::WidgetTree<crate::WidgetKind>,
    root: WidgetId,
) -> Option<Result<String, ()>> {
    if !is_prompt(tree, root) {
        return None;
    }
    let query = command_palette_state(tree, root).ok()?.query().to_owned();
    let Ok(name) = aurora_doc::validate_path_name(&query) else {
        let message = refusal_message().to_owned();
        let row = vec![CommandEntry::new(ROW_NAME, message.clone())];
        if let Err(err) = set_command_palette_commands(tree, root, row) {
            tracing::warn!(?err, "failed to show a refused path name");
        }
        if let Err(err) = set_command_palette_message(tree, root, Some(message)) {
            tracing::warn!(?err, "failed to describe a refused path name");
        }
        return Some(Err(()));
    };
    Some(Ok(name))
}

// ------------------------------------------------------- pure decisions

/// What the Pen tool does with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PenKey {
    Ignore,
    End,
    Cancel,
    Backspace,
    Delete,
}

/// [`App::pen_key_now`]'s decision as a free function: the keys are the
/// Pen's only while it is the tool, no modal is open, no modifier is held
/// and there is something for the key to act on.
pub(crate) fn pen_key_action(
    tool: aurora_ui::Tool,
    modal_open: bool,
    modifiers: aurora_widgets::shortcut::Modifiers,
    drawing: bool,
    selected: bool,
    key: Key,
) -> PenKey {
    if tool != aurora_ui::Tool::Pen
        || modal_open
        || modifiers.control
        || modifiers.alt
        || modifiers.meta
    {
        return PenKey::Ignore;
    }
    match key {
        Key::Named(NamedKey::Enter) if drawing => PenKey::End,
        Key::Named(NamedKey::Escape) if drawing => PenKey::Cancel,
        Key::Named(NamedKey::Backspace) if drawing => PenKey::Backspace,
        Key::Named(NamedKey::Delete) if selected => PenKey::Delete,
        _ => PenKey::Ignore,
    }
}

/// The selection a path makes, or the reason it cannot.
pub(crate) fn selection_of_path(
    paths: &PathSet,
    name: Option<&str>,
    canvas: (u32, u32),
) -> Result<Selection, &'static str> {
    let Some(path) = name.and_then(|n| paths.get(n)) else {
        return Err(NO_PATH_NOTICE);
    };
    match aurora_doc::selection_from_path(path, canvas) {
        Ok(Some(selection)) => Ok(selection),
        Ok(None) => Err(EMPTY_PATH_NOTICE),
        Err(_) => Err(SELECTION_FAILED_NOTICE),
    }
}

/// Makes the active selection from the path `name` as ONE selection step.
pub(crate) fn make_selection(
    workspace: &mut aurora_ui::Workspace,
    doc: &mut DocumentSession,
    name: Option<&str>,
) -> Result<(), &'static str> {
    let selection = selection_of_path(doc.selection.paths(), name, doc.canvas_size)?;
    let before = doc.selection.active();
    doc.selection.select(selection);
    crate::record_selection_step(
        workspace,
        &mut doc.undo_order,
        &mut doc.history,
        &mut doc.pixel_history,
        MAKE_SELECTION_LABEL,
        before,
        doc.selection.active(),
    );
    Ok(())
}

pub(crate) const MAKE_SELECTION_LABEL: &str = "Make Selection from Path";

/// Records a finished Pen gesture or path command as one step.
pub(crate) fn record_step(
    workspace: &mut aurora_ui::Workspace,
    doc: &mut DocumentSession,
    step: pen::PenStep,
) {
    let (path_step, label) = step.into_parts();
    if doc
        .undo_order
        .record_path_step(path_step, label, &mut doc.history, &mut doc.pixel_history)
    {
        crate::refresh_history_panel(workspace, &doc.undo_order);
    }
}

/// An edit of the path set as a step: applies `edit` to a copy, and when it
/// succeeds and changes something installs it and returns the step.
pub(crate) fn edit_paths(
    paths: &mut PathSet,
    label: &'static str,
    edit: impl FnOnce(&mut PathSet) -> Result<(), aurora_doc::DocError>,
) -> Result<Option<pen::PenStep>, aurora_doc::DocError> {
    let before = paths.clone();
    let mut after = before.clone();
    edit(&mut after)?;
    after.validate()?;
    if after == before {
        return Ok(None);
    }
    *paths = after.clone();
    Ok(Some(pen::PenStep {
        label,
        before,
        after,
    }))
}

/// The Paths panel's rows (every path's name) and the highlighted one, for
/// the per-turn sync.
pub(crate) fn sync_paths(
    workspace: &mut aurora_ui::Workspace,
    scales: &aurora_theme::Scales,
    doc: &DocumentSession,
    active: Option<&str>,
) -> bool {
    let names: Vec<String> = doc.selection.paths().names().map(str::to_owned).collect();
    let selected = active.filter(|n| doc.selection.paths().get(n).is_some());
    let selected_empty = selected
        .and_then(|n| doc.selection.paths().get(n))
        .is_some_and(aurora_doc::Path::is_empty);
    let panel = workspace.paths;
    let mut state = workspace.paths_state.clone();
    let result = aurora_ui::sync_paths_panel(
        &mut workspace.tree,
        panel,
        scales,
        &mut state,
        &names,
        selected,
        selected_empty,
    );
    workspace.paths_state = state;
    match result {
        Ok(changed) => changed,
        Err(err) => {
            tracing::warn!(?err, "failed to sync the Paths panel");
            false
        }
    }
}

// --------------------------------------------------------------- App glue

impl App {
    fn pen_notice(&mut self, text: &'static str) {
        self.lock_notice = Some((LockRefusal::Path(text), self.doc.active_layer));
        self.sync_status_bar_now();
        self.needs_redraw = true;
    }

    fn pen_hit_radius(&self) -> f32 {
        f32::from(u16::try_from(self.scales.spacing.sm).unwrap_or(12))
    }

    fn pen_handle_px(&self) -> u32 {
        let side = f64::from(self.scales.spacing.xs)
            * f64::from(crate::guarded_scale_factor(self.scale_factor));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let px = side.round().max(1.0) as u32;
        px
    }

    /// Applies what a Pen operation reported: the step, the notice.
    fn pen_apply(&mut self, outcome: PenOutcome) {
        if let Some(step) = outcome.step {
            record_step(&mut self.workspace, &mut self.doc, step);
        }
        if let Some(text) = outcome.refused {
            self.pen_notice(text);
        }
        if outcome.redraw {
            self.needs_redraw = true;
        }
    }

    /// A primary press on the canvas with the Pen tool.
    pub(crate) fn pen_press(&mut self, canvas_point: (f32, f32)) {
        self.pen.adopt_document(self.doc.id);
        let doc_point = self.doc.canvas_view.to_document(canvas_point);
        let zoom = self.doc.canvas_view.zoom();
        let close_radius = if zoom.is_finite() && zoom > 0.0 {
            crate::lasso_tools::POLYGON_CLOSE_TOLERANCE_PX / zoom
        } else {
            0.0
        };
        let hit = self
            .pen
            .active()
            .and_then(|name| self.doc.selection.paths().get(name))
            .and_then(|path| {
                aurora_ui::path_hit(
                    path,
                    self.pen.selected(),
                    &self.doc.canvas_view,
                    canvas_point,
                    self.pen_hit_radius(),
                )
            });
        let alt = self.modifiers.alt;
        let outcome = self.pen.press(
            self.doc.selection.paths_mut(),
            doc_point,
            hit,
            close_radius,
            alt,
        );
        self.pen_apply(outcome);
    }

    /// The pointer moved with a Pen gesture live; returns whether it did.
    pub(crate) fn pen_pointer_moved(&mut self, canvas_point: (f32, f32)) -> bool {
        if !self.pen.dragging() {
            return false;
        }
        let doc_point = self.doc.canvas_view.to_document(canvas_point);
        if self.pen.drag(self.doc.selection.paths_mut(), doc_point) {
            self.needs_redraw = true;
        }
        true
    }

    /// The primary button was released: a live Pen gesture ends.
    pub(crate) fn pen_pointer_released(&mut self) {
        if self.pen.dragging() {
            let outcome = self.pen.release(self.doc.selection.paths_mut());
            self.pen_apply(outcome);
        }
    }

    /// Enter, Escape, Backspace and Delete for the Pen, offered after the
    /// focused widgets. Returns whether the key was used.
    pub(crate) fn pen_key_now(&mut self, key: Key) -> bool {
        let modal = self.dialog.is_some()
            || self.command_palette.is_some()
            || crate::crop_app::text_field_focused(&self.workspace.tree, self.focus.focused());
        let action = pen_key_action(
            self.tool,
            modal,
            self.modifiers,
            self.pen.drawing().is_some(),
            self.pen.selected().is_some(),
            key,
        );
        let outcome = match action {
            PenKey::Ignore => return false,
            PenKey::End => {
                self.pen.end_subpath();
                PenOutcome {
                    redraw: true,
                    ..PenOutcome::default()
                }
            }
            PenKey::Cancel => self.pen.cancel_subpath(self.doc.selection.paths_mut()),
            PenKey::Backspace => self.pen.remove_last_point(self.doc.selection.paths_mut()),
            PenKey::Delete => self.pen.delete_selected(self.doc.selection.paths_mut()),
        };
        self.pen_apply(outcome);
        true
    }

    /// Run once per loop turn: a Pen gesture or subpath that no longer
    /// belongs to the active tool or document ends.
    pub(crate) fn pen_drop_if_stale(&mut self) {
        if self.pen.adopt_document(self.doc.id) {
            self.needs_redraw = true;
        }
        if self.tool != aurora_ui::Tool::Pen
            && (self.pen.dragging()
                || self.pen.drawing().is_some()
                || self.pen.selected().is_some())
        {
            self.pen_pointer_released();
            self.pen.reset_editing();
            self.needs_redraw = true;
        }
    }

    /// Puts the path set an undo, redo or History jump has just put in force
    /// into the live document; returns whether there was one.
    pub(crate) fn apply_pending_paths_now(&mut self) -> bool {
        if self.doc.undo_order.take_paths_lost() {
            self.pen_notice(PATHS_LOST_NOTICE);
        }
        let Some(paths) = self.doc.undo_order.take_pending_paths() else {
            return false;
        };
        self.pen.paths_replaced(&paths);
        self.doc.selection.set_paths(paths);
        self.needs_redraw = true;
        true
    }

    /// The Pen's overlay for this frame (empty unless the Pen is the tool
    /// and has an active path).
    pub(crate) fn pen_overlay_runs_now(&self) -> Vec<aurora_widgets::widgets::OutlineRun> {
        let (aurora_ui::Tool::Pen, Some(name), Some(size)) = (
            self.tool,
            self.pen.active(),
            crate::canvas_area_logical_size(&self.workspace),
        ) else {
            return Vec::new();
        };
        let Some(path) = self.doc.selection.paths().get(name) else {
            return Vec::new();
        };
        aurora_ui::path_overlay_runs(
            path,
            self.pen.selected(),
            &self.doc.canvas_view,
            size,
            crate::guarded_scale_factor(self.scale_factor),
            self.pen_handle_px(),
        )
    }

    /// Keeps the Paths panel in line with the document, every loop turn.
    pub(crate) fn sync_paths_now(&mut self) {
        let active = self.pen.active().map(str::to_owned);
        if sync_paths(
            &mut self.workspace,
            &self.scales,
            &self.doc,
            active.as_deref(),
        ) {
            self.relayout_after_gallery();
        }
    }

    /// A press on a Paths row selects it (and makes it the Pen's path); a
    /// second press on the same row within the double-click time asks for a
    /// new name. Returns whether the press was on a row.
    pub(crate) fn press_paths_row(&mut self, position: (f32, f32)) -> bool {
        if self.dialog.is_some() || self.command_palette.is_some() {
            return false;
        }
        let Some(hit) = self.workspace.tree.hit_test(position) else {
            return false;
        };
        let Some(name) = self
            .workspace
            .paths_state
            .row_at(&self.workspace.tree, hit)
            .map(str::to_owned)
        else {
            return false;
        };
        let row = self.workspace.paths_state.id_of(&name);
        let now = Instant::now();
        let double = row.is_some()
            && crate::channels_app::is_double_click(self.path_click, row.unwrap_or(hit), now);
        self.path_click = if double {
            None
        } else {
            row.map(|id| (id, now))
        };
        self.select_path(&name);
        if double {
            self.run_paths_command(PathsCommand::Rename);
        }
        true
    }

    /// Makes `name` the Pen's active path and highlights its row.
    pub(crate) fn select_path(&mut self, name: &str) {
        self.pen.set_active(Some(name.to_owned()));
        self.sync_paths_now();
        self.needs_redraw = true;
    }

    /// An AT `Click` or Enter on a Paths row.
    pub(crate) fn click_paths_row(&mut self, row: WidgetId) {
        if let Some(name) = self.workspace.paths_state.name_of(row).map(str::to_owned) {
            self.select_path(&name);
        }
    }

    /// One Path actions toolbar button.
    pub(crate) fn run_paths_action(&mut self, action: aurora_ui::PathsAction) {
        let command = match action {
            aurora_ui::PathsAction::MakeSelection => PathsCommand::MakeSelection,
            aurora_ui::PathsAction::NewPath => PathsCommand::New,
            aurora_ui::PathsAction::DeletePath => PathsCommand::Delete,
        };
        self.run_paths_command(command);
        self.relayout_after_gallery();
    }

    /// Runs a Paths command.
    pub(crate) fn run_paths_command(&mut self, command: PathsCommand) {
        let active = self.pen.active().map(str::to_owned);
        match command {
            PathsCommand::MakeSelection => {
                if self.quick_mask_refuses_command() {
                    return;
                }
                crate::lasso_tools::cancel_selection_gesture(
                    &mut self.drag,
                    &mut self.doc.selection,
                );
                match make_selection(&mut self.workspace, &mut self.doc, active.as_deref()) {
                    Ok(()) => self.needs_redraw = true,
                    Err(text) => self.pen_notice(text),
                }
            }
            PathsCommand::New => {
                let name = self.doc.selection.paths().next_free_name();
                let result = edit_paths(self.doc.selection.paths_mut(), NEW_PATH_LABEL, |set| {
                    set.add(&name).map(|_| ())
                });
                match result {
                    Ok(step) => {
                        if let Some(step) = step {
                            record_step(&mut self.workspace, &mut self.doc, step);
                        }
                        self.pen.set_active(Some(name));
                    }
                    Err(_) => self.pen_notice(PATHS_FULL_NOTICE),
                }
            }
            PathsCommand::Delete => {
                let Some(name) = active else {
                    self.pen_notice(NO_PATH_NOTICE);
                    return;
                };
                let result = edit_paths(self.doc.selection.paths_mut(), DELETE_PATH_LABEL, |set| {
                    set.delete(&name)
                });
                if let Ok(Some(step)) = result {
                    record_step(&mut self.workspace, &mut self.doc, step);
                    self.pen.set_active(None);
                }
            }
            PathsCommand::Rename | PathsCommand::SaveWork => {
                let target = if command == PathsCommand::SaveWork {
                    if self.doc.selection.paths().work().is_none() {
                        self.pen_notice(NO_WORK_PATH_NOTICE);
                        return;
                    }
                    Some(WORK_PATH_NAME.to_owned())
                } else {
                    active.filter(|n| self.doc.selection.paths().get(n).is_some())
                };
                let Some(target) = target else {
                    self.pen_notice(NO_PATH_NOTICE);
                    return;
                };
                if open_prompt(
                    &mut self.workspace,
                    &mut self.focus,
                    &mut self.command_palette,
                ) {
                    self.path_name_target = Some(target);
                    self.push_accessibility();
                    self.needs_redraw = true;
                }
            }
            PathsCommand::Name(new) => {
                let Some(old) = self.path_name_target.take() else {
                    return;
                };
                let label = if old == WORK_PATH_NAME {
                    SAVE_WORK_LABEL
                } else {
                    RENAME_PATH_LABEL
                };
                let mut renamed = None;
                let result = edit_paths(self.doc.selection.paths_mut(), label, |set| {
                    renamed = Some(set.rename(&old, &new)?);
                    Ok(())
                });
                match result {
                    Ok(step) => {
                        if let Some(step) = step {
                            record_step(&mut self.workspace, &mut self.doc, step);
                        }
                        if let Some(name) = renamed {
                            self.pen.set_active(Some(name));
                        }
                    }
                    Err(_) => self.pen_notice(NAME_TAKEN_NOTICE),
                }
            }
        }
        self.needs_redraw = true;
    }
}

#[cfg(test)]
#[path = "paths_app_tests.rs"]
mod tests;
