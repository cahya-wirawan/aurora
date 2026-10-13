//! Save Selection, Load Selection and Delete Saved Selection (0.218.0), the
//! app half of `SelectionSet`'s named selections.
//!
//! **Save Selection...** opens the palette's own prompt widget (the one
//! Select > Modify and "Save Workspace As..." use) for a name, validated by
//! `aurora_doc::validate_selection_name` (trimmed, non-empty, no control or
//! text-direction characters, at most 64 characters); a refused name keeps
//! the prompt open with the reason as its row and accessible description.
//! A name already in use is **replaced** (the model's `save_active`
//! semantics) without a second confirmation. The caps (64 selections,
//! 128 MiB of mask coverage) refuse with a calm status-bar notice.
//!
//! **Load Selection: name** is a dynamic palette entry (listed when a key
//! opens the palette, like the workspace ones) and records ONE undo step
//! "Load Selection" through `record_selection_step`. **Delete Saved
//! Selection: name** is its sibling. Saving and deleting are document
//! metadata: not undoable, never dirty (they move only
//! `UndoOrder::revision`, which the park autosave keys on, so they are
//! autosaved), and written by the next `.aur` save.

use aurora_widgets::widgets::set_command_palette_query;
use aurora_widgets::widgets::{
    CommandEntry, command_palette_state, insert_command_prompt, set_command_palette_commands,
    set_command_palette_message,
};

use crate::{FocusManager, LockRefusal, WidgetId, command_palette_style};

/// The palette command that opens the name prompt.
pub(crate) const COMMAND_SAVE: &str = "select.save";
/// Its palette title.
pub(crate) const SAVE_TITLE: &str = "Save Selection\u{2026}";
/// The History row of a load.
pub(crate) const LOAD_STEP_LABEL: &str = "Load Selection";
/// The prompt's accessible name.
pub(crate) const PROMPT_LABEL: &str = "Selection Name";
/// The id of the prompt's one row.
const ROW_SAVE: &str = "select.save.apply";
const LOAD_PREFIX: &str = "select.load:";
const DELETE_PREFIX: &str = "select.delete_saved:";

/// What a saved-selection command asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SavedSelectionCommand {
    /// Open the name prompt.
    Prompt,
    /// Save the active selection under this (validated) name.
    Save(String),
    Load(String),
    Delete(String),
}

/// The command the palette id `id` stands for.
pub(crate) fn command_for(id: &str) -> Option<SavedSelectionCommand> {
    if id == COMMAND_SAVE {
        return Some(SavedSelectionCommand::Prompt);
    }
    if let Some(name) = id.strip_prefix(LOAD_PREFIX) {
        return Some(SavedSelectionCommand::Load(name.to_owned()));
    }
    id.strip_prefix(DELETE_PREFIX)
        .map(|name| SavedSelectionCommand::Delete(name.to_owned()))
}

/// The dynamic palette entries for `set`: Load and Delete per saved name.
pub(crate) fn palette_entries(set: &aurora_doc::SelectionSet) -> Vec<CommandEntry> {
    let mut entries = Vec::new();
    for (name, _) in set.saved_sorted() {
        entries.push(CommandEntry::new(
            format!("{LOAD_PREFIX}{name}"),
            format!("Load Selection: {name}"),
        ));
    }
    for (name, _) in set.saved_sorted() {
        entries.push(CommandEntry::new(
            format!("{DELETE_PREFIX}{name}"),
            format!("Delete Saved Selection: {name}"),
        ));
    }
    entries
}

/// Appends the dynamic entries to a palette a key just opened (a no-op when
/// it was already open, is closed or is a prompt).
pub(crate) fn add_entries_if_opened(
    workspace: &mut aurora_ui::Workspace,
    was_open: bool,
    palette: Option<WidgetId>,
    set: &aurora_doc::SelectionSet,
) -> bool {
    let Some(root) = palette else {
        return false;
    };
    if was_open || set.saved_count() == 0 {
        return false;
    }
    let Ok(state) = command_palette_state(&workspace.tree, root) else {
        return false;
    };
    if !state.is_filtering() {
        return false;
    }
    let mut commands = state.commands().to_vec();
    commands.extend(palette_entries(set));
    if let Err(err) = set_command_palette_commands(&mut workspace.tree, root, commands) {
        tracing::warn!(?err, "failed to add the saved selections to the palette");
        return false;
    }
    true
}

/// A refused name's sentence for the prompt.
pub(crate) fn refusal_message(error: aurora_doc::SelectionNameError) -> String {
    match error {
        aurora_doc::SelectionNameError::Empty => "Type a name for the selection.".to_owned(),
        aurora_doc::SelectionNameError::InvalidCharacter => {
            "A selection name can't contain control or text-direction characters.".to_owned()
        }
        aurora_doc::SelectionNameError::TooLong => format!(
            "A selection name can be at most {} characters long.",
            aurora_doc::SELECTION_NAME_MAX_CHARS
        ),
    }
}

fn prompt_entries(query: &str) -> Vec<CommandEntry> {
    let name = query.trim();
    let title = if name.is_empty() {
        "Save Selection".to_owned()
    } else {
        format!("Save Selection \u{201c}{name}\u{201d}")
    };
    vec![CommandEntry::new(ROW_SAVE, title)]
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
            tracing::warn!(?err, "failed to open the selection name prompt");
            return false;
        }
    };
    if let Err(err) = workspace.tree.set_style(root, command_palette_style()) {
        tracing::warn!(?err, "failed to size the selection name prompt");
    }
    if let Err(err) = set_command_palette_query(&mut workspace.tree, root, "") {
        tracing::warn!(?err, "failed to clear the selection name prompt");
    }
    if let Err(err) = focus.focus(&mut workspace.tree, root) {
        tracing::warn!(?err, "failed to focus the selection name prompt");
    }
    *palette = Some(root);
    true
}

fn is_prompt(tree: &aurora_widgets::WidgetTree<crate::WidgetKind>, root: WidgetId) -> bool {
    command_palette_state(tree, root).is_ok_and(|state| {
        !state.is_filtering() && state.commands().first().is_some_and(|c| c.id == ROW_SAVE)
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
        tracing::warn!(?err, "failed to update the selection name prompt");
    }
    if let Err(err) = set_command_palette_message(tree, root, None) {
        tracing::warn!(?err, "failed to clear the selection name prompt's message");
    }
    true
}

/// The prompt's Enter. `None` when `root` is not this prompt;
/// `Some(Ok(name))` for a valid name (the caller closes the prompt and
/// saves); `Some(Err(()))` for a refused one, which stays open with the
/// reason in its row and description.
pub(crate) fn prompt_enter(
    tree: &mut aurora_widgets::WidgetTree<crate::WidgetKind>,
    root: WidgetId,
) -> Option<Result<String, ()>> {
    if !is_prompt(tree, root) {
        return None;
    }
    let query = command_palette_state(tree, root).ok()?.query().to_owned();
    match aurora_doc::validate_selection_name(&query) {
        Ok(name) => Some(Ok(name)),
        Err(error) => {
            let message = refusal_message(error);
            let row = vec![CommandEntry::new(ROW_SAVE, message.clone())];
            if let Err(err) = set_command_palette_commands(tree, root, row) {
                tracing::warn!(?err, "failed to show a refused selection name");
            }
            if let Err(err) = set_command_palette_message(tree, root, Some(message)) {
                tracing::warn!(?err, "failed to describe a refused selection name");
            }
            Some(Err(()))
        }
    }
}

/// What a Save, Load or Delete did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreOutcome {
    /// The document's saved selections changed (a metadata change).
    MetadataChanged,
    /// The active selection changed and one step was recorded.
    Loaded,
    /// Nothing changed.
    Unchanged,
    /// Nothing changed; a calm status-bar notice.
    Refused(LockRefusal),
}

/// Saves the active selection as `name`.
pub(crate) fn save(
    undo_order: &mut crate::UndoOrder,
    selection: &mut aurora_doc::SelectionSet,
    name: &str,
) -> StoreOutcome {
    match selection.save_active(name) {
        Ok(()) => {
            undo_order.note_selection_metadata_change();
            StoreOutcome::MetadataChanged
        }
        Err(aurora_doc::DocError::NoActiveSelection) => {
            StoreOutcome::Refused(LockRefusal::SavedSelectionNeedsSelection)
        }
        Err(aurora_doc::DocError::SavedSelectionsFull) => {
            StoreOutcome::Refused(LockRefusal::SavedSelectionsFull)
        }
        Err(_) => StoreOutcome::Refused(LockRefusal::SavedSelectionsTooLarge),
    }
}

/// Deletes the saved selection `name`.
pub(crate) fn delete(
    undo_order: &mut crate::UndoOrder,
    selection: &mut aurora_doc::SelectionSet,
    name: &str,
) -> StoreOutcome {
    if selection.delete_saved(name).is_ok() {
        undo_order.note_selection_metadata_change();
        StoreOutcome::MetadataChanged
    } else {
        StoreOutcome::Unchanged
    }
}

/// Makes the saved selection `name` the active one as ONE undo step.
#[allow(clippy::too_many_arguments)]
pub(crate) fn load(
    workspace: &mut aurora_ui::Workspace,
    undo_order: &mut crate::UndoOrder,
    history: &mut aurora_doc::History,
    pixel_history: &mut aurora_brush::PixelHistory,
    selection: &mut aurora_doc::SelectionSet,
    name: &str,
) -> StoreOutcome {
    let before = selection.active();
    if selection.load(name).is_err() {
        return StoreOutcome::Unchanged;
    }
    if crate::record_selection_step(
        workspace,
        undo_order,
        history,
        pixel_history,
        LOAD_STEP_LABEL,
        before,
        selection.active(),
    ) {
        StoreOutcome::Loaded
    } else {
        StoreOutcome::Unchanged
    }
}
