//! Workspace presets (0.168.0) — named, saved panel layouts to switch
//! between, Photoshop-style.
//!
//! One preset is built in, [`ESSENTIALS`]: the default layout (every panel
//! docked in the default order and grouping, the rail expanded at
//! [`aurora_ui::RAIL_WIDTH_DEFAULT`], the default tab selected, nothing
//! collapsed). The user's own presets are captured from the live layout
//! with "Save Workspace As…" ([`WorkspaceCommand::PromptSaveAs`]), which
//! asks for a name in a *prompt* — the command palette's own widget with
//! its own name and no filtering (`aurora_widgets::widgets::
//! insert_command_prompt`), so typing, paste, Escape, Enter, focus on open
//! and the `Role::TextInput` accessibility shape are the palette's,
//! already tested, rather than a second text-entry path inside a dialog.
//!
//! A preset is a [`WorkspaceLayout`] — the same value the live layout file
//! holds: the dock arrangement (floats and their stacking), rail width,
//! rail collapse, each panel's collapse and the selected tabs. Not tool
//! settings, not document state; nothing here reaches `History`.
//!
//! **The active preset.** Switching ([`WorkspaceCommand::Switch`]) applies
//! a preset and makes it active; later edits change the live layout only,
//! never the saved preset, until it is saved again. "Reset `name`"
//! ([`WorkspaceCommand::ResetActive`]) re-applies the active preset's
//! saved state. "Reset Panel Layout" is an alias of switching to
//! Essentials. Deleting the active preset makes Essentials active and
//! leaves the live layout alone.
//!
//! **Persistence: a separate file**, `workspace-presets.postcard`, next to
//! the live layout file ([`presets_path_for`]), rather than a sixth
//! `WorkspaceLayout` version: the live layout's decode chain stays exactly
//! as 0.167.0 left it (an older build still reads it), a damaged presets
//! file can never cost the user their live layout, and each preset's
//! layout is stored as its own byte string decoded with the live layout's
//! own fallback chain ([`super::decode_workspace_layout`]), so one damaged
//! preset is dropped alone. The file starts with a magic and a version; it
//! is written to a temporary file, synced and renamed over the old one
//! ([`save_presets`]) — unlike the live layout file, which is written in
//! place (unchanged here).

use std::path::{Path, PathBuf};

use aurora_theme::Scales;
use aurora_widgets::widgets::{
    CommandEntry, command_palette_state, insert_command_prompt, set_command_palette_commands,
    set_command_palette_message, set_command_palette_query,
};

use super::{
    FocusManager, WidgetId, WorkspaceLayout, apply_saved_collapse, arrangement_from_saved,
    command_palette_style, decode_workspace_layout, default_saved_dock, refocus_out_of_hidden,
    saved_dock, saved_float_stack, workspace_layout,
};

/// The built-in preset's name.
pub(crate) const ESSENTIALS: &str = "Essentials";
/// The longest workspace name accepted, in Unicode scalar values — an
/// engineering cap (a palette row has to show it), not a design token.
pub(crate) const WORKSPACE_NAME_MAX_CHARS: usize = 64;
/// The name prompt's accessible name.
pub(crate) const WORKSPACE_NAME_PROMPT_LABEL: &str = "Workspace Name";

/// "Save Workspace As…": opens the name prompt.
pub(crate) const COMMAND_WORKSPACE_SAVE_AS: &str = "workspace.save_as";
/// "Reset `active`": re-applies the active preset's saved state.
pub(crate) const COMMAND_WORKSPACE_RESET_ACTIVE: &str = "workspace.reset_active";
/// "Workspace: `name`" ids are this prefix plus the name.
pub(crate) const COMMAND_WORKSPACE_SWITCH_PREFIX: &str = "workspace.switch/";
/// "Delete Workspace: `name`" ids are this prefix plus the name.
pub(crate) const COMMAND_WORKSPACE_DELETE_PREFIX: &str = "workspace.delete/";
/// The name prompt's row: save under the typed name.
pub(crate) const COMMAND_WORKSPACE_NAME_SAVE: &str = "workspace.name.save";
/// The name prompt's row once the typed name is taken: replace it.
pub(crate) const COMMAND_WORKSPACE_NAME_REPLACE: &str = "workspace.name.replace";

/// The most user presets kept (0.168.0 review J2, an engineering cap):
/// Save As refuses a new name beyond it, and decoding stops there.
pub(crate) const MAX_WORKSPACE_PRESETS: usize = 100;
/// The most bytes of the presets file read (review J2, an engineering
/// cap far above `MAX_WORKSPACE_PRESETS` real presets): a larger file is
/// decoded from its first this-many bytes only.
pub(crate) const PRESETS_FILE_MAX_BYTES: u64 = 1 << 20;

const PRESETS_FILE_NAME: &str = "workspace-presets.postcard";
const PRESETS_MAGIC: [u8; 4] = *b"AWSP";
const PRESETS_VERSION: u32 = 1;

/// One user preset.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorkspacePreset {
    pub(crate) name: String,
    pub(crate) layout: WorkspaceLayout,
}

/// The user's presets and which preset is active (`None` is Essentials).
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct WorkspacePresets {
    user: Vec<WorkspacePreset>,
    active: Option<String>,
}

impl WorkspacePresets {
    /// The active preset's name ([`ESSENTIALS`] when no user preset is).
    pub(crate) fn active_name(&self) -> &str {
        self.active.as_deref().unwrap_or(ESSENTIALS)
    }

    /// The user's presets, in the order they were first saved.
    pub(crate) fn user(&self) -> &[WorkspacePreset] {
        &self.user
    }

    fn user_index(&self, name: &str) -> Option<usize> {
        let wanted = name.to_lowercase();
        self.user
            .iter()
            .position(|preset| preset.name.to_lowercase() == wanted)
    }

    /// The saved layout `name` names (case-insensitive), Essentials
    /// included; `None` for a name that is no preset.
    pub(crate) fn layout_of(&self, name: &str) -> Option<WorkspaceLayout> {
        if is_essentials(name) {
            return Some(essentials_layout());
        }
        self.user_index(name)
            .and_then(|index| self.user.get(index))
            .map(|preset| preset.layout.clone())
    }

    /// Whether `name` is a user preset's name (case-insensitive).
    pub(crate) fn has_user(&self, name: &str) -> bool {
        self.user_index(name).is_some()
    }

    /// Saves `layout` as `name` (already validated) and makes it active;
    /// a user preset of the same name (case-insensitive) is overwritten,
    /// taking the new spelling. Returns whether one was overwritten.
    fn save(&mut self, name: String, layout: WorkspaceLayout) -> bool {
        let existing = self
            .user_index(&name)
            .and_then(|index| self.user.get_mut(index));
        let replaced = if let Some(existing) = existing {
            existing.name.clone_from(&name);
            existing.layout = layout;
            true
        } else {
            self.user.push(WorkspacePreset {
                name: name.clone(),
                layout,
            });
            false
        };
        self.active = Some(name);
        replaced
    }

    /// Removes the user preset `name`; Essentials and unknown names are
    /// refused (`false`). Deleting the active preset makes Essentials
    /// active.
    fn delete(&mut self, name: &str) -> bool {
        if is_essentials(name) {
            return false;
        }
        let Some(index) = self.user_index(name) else {
            return false;
        };
        let removed = self.user.remove(index);
        if self
            .active
            .as_deref()
            .is_some_and(|active| active.to_lowercase() == removed.name.to_lowercase())
        {
            self.active = None;
        }
        true
    }

    /// Makes `name` active, in its saved spelling. `false` for no preset.
    fn set_active(&mut self, name: &str) -> bool {
        if is_essentials(name) {
            self.active = None;
            return true;
        }
        match self.user_index(name).and_then(|index| self.user.get(index)) {
            Some(preset) => {
                self.active = Some(preset.name.clone());
                true
            }
            None => false,
        }
    }
}

fn is_essentials(name: &str) -> bool {
    name.trim().to_lowercase() == ESSENTIALS.to_lowercase()
}

/// Why a workspace name was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceNameError {
    Empty,
    /// A control, line/paragraph separator or bidi override/isolate
    /// character (`aurora_widgets::widgets::is_insertable_char` refuses
    /// it), 0.168.0 review J1.
    InvalidCharacter,
    TooLong,
    Reserved,
}

impl WorkspaceNameError {
    /// What the prompt shows and announces.
    pub(crate) fn message(self) -> String {
        match self {
            Self::Empty => "Type a name for the workspace.".to_owned(),
            Self::InvalidCharacter => {
                "A workspace name can't contain control or text-direction characters.".to_owned()
            }
            Self::TooLong => format!(
                "A workspace name can be at most {WORKSPACE_NAME_MAX_CHARS} characters long."
            ),
            Self::Reserved => {
                format!(
                    "\u{201c}{ESSENTIALS}\u{201d} is the built-in workspace; choose another name."
                )
            }
        }
    }
}

/// A typed name, trimmed and checked: non-empty, only characters a text
/// field would insert (`aurora_widgets::widgets::is_insertable_char`:
/// no controls, no line/paragraph separators, no bidi overrides or
/// isolates U+202A–U+202E, U+2066–U+2069), at most
/// [`WORKSPACE_NAME_MAX_CHARS`], not "Essentials" in any case. A name
/// read from the presets file goes through the same check.
pub(crate) fn validate_workspace_name(raw: &str) -> Result<String, WorkspaceNameError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(WorkspaceNameError::Empty);
    }
    if !name
        .chars()
        .all(aurora_widgets::widgets::is_insertable_char)
    {
        return Err(WorkspaceNameError::InvalidCharacter);
    }
    if name.chars().count() > WORKSPACE_NAME_MAX_CHARS {
        return Err(WorkspaceNameError::TooLong);
    }
    if is_essentials(name) {
        return Err(WorkspaceNameError::Reserved);
    }
    Ok(name.to_owned())
}

/// The built-in Essentials layout: the default arrangement, the rail
/// expanded at its default width, nothing collapsed, the default tab.
pub(crate) fn essentials_layout() -> WorkspaceLayout {
    let tab = u32::try_from(aurora_ui::PANEL_GROUP_TAB_DEFAULT).unwrap_or(0);
    WorkspaceLayout {
        rail_width: aurora_ui::RAIL_WIDTH_DEFAULT,
        layers_collapsed: false,
        properties_collapsed: false,
        history_collapsed: false,
        panel_group_tab: tab,
        rail_collapsed: false,
        dock: default_saved_dock(tab),
        float_stack: Vec::new(),
    }
}

/// Applies `layout` to the live workspace, keeping focus where it can:
/// rail width (clamped), the arrangement (repaired) through
/// `aurora_ui::apply_dock_arrangement_keeping_focus`, each slot's collapse,
/// the rail's collapse, then the hidden-focus repair. Floats are clamped
/// into the window by the next layout (`aurora_ui::sync_floating_frames`).
/// Returns whether the arrangement changed.
pub(crate) fn apply_layout_keeping_focus(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    layout: &WorkspaceLayout,
    scales: &Scales,
) -> bool {
    if let Err(err) = aurora_ui::set_rail_width(
        &mut workspace.tree,
        workspace.rail,
        workspace.divider,
        layout.rail_width,
    ) {
        tracing::warn!(?err, "failed to apply a workspace's rail width");
    }
    let (arrangement, repaired) = arrangement_from_saved(&layout.dock, &layout.float_stack);
    if repaired {
        tracing::warn!("a workspace's panel arrangement was damaged; repaired");
    }
    let changed = match aurora_ui::apply_dock_arrangement_keeping_focus(
        workspace,
        focus,
        &arrangement,
        scales,
    ) {
        Ok(changed) => changed,
        Err(err) => {
            tracing::warn!(?err, "failed to apply a workspace's panel arrangement");
            false
        }
    };
    apply_saved_collapse(workspace, layout);
    if let Err(err) = aurora_ui::set_rail_collapsed(workspace, layout.rail_collapsed) {
        tracing::warn!(?err, "failed to apply a workspace's rail collapse");
    }
    refocus_out_of_hidden(workspace, focus);
    changed
}

/// The live layout, as a preset would capture it.
pub(crate) fn live_layout(workspace: &aurora_ui::Workspace) -> WorkspaceLayout {
    let rail_width = aurora_ui::rail_width(&workspace.tree, workspace.rail)
        .unwrap_or(aurora_ui::RAIL_WIDTH_DEFAULT);
    workspace_layout(workspace, rail_width)
}

/// A workspace command, resolved from a palette id
/// ([`workspace_command_for`]) or from the name prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceCommand {
    /// "Workspace: `name`".
    Switch(String),
    /// "Delete Workspace: `name`".
    Delete(String),
    /// "Reset `active`".
    ResetActive,
    /// "Save Workspace As…": open the name prompt.
    PromptSaveAs,
    /// The prompt's Enter on a valid name; `replace` once the user has
    /// been told the name is taken.
    Save { name: String, replace: bool },
}

/// Resolves a palette id to a workspace command, `None` for any other id.
pub(crate) fn workspace_command_for(id: &str) -> Option<WorkspaceCommand> {
    if id == COMMAND_WORKSPACE_SAVE_AS {
        return Some(WorkspaceCommand::PromptSaveAs);
    }
    if id == COMMAND_WORKSPACE_RESET_ACTIVE {
        return Some(WorkspaceCommand::ResetActive);
    }
    if let Some(name) = id.strip_prefix(COMMAND_WORKSPACE_SWITCH_PREFIX) {
        return Some(WorkspaceCommand::Switch(name.to_owned()));
    }
    id.strip_prefix(COMMAND_WORKSPACE_DELETE_PREFIX)
        .map(|name| WorkspaceCommand::Delete(name.to_owned()))
}

/// The palette's workspace entries, built from the presets each time the
/// palette opens: "Workspace: `name`" for Essentials and each user preset,
/// "Reset `active`", "Save Workspace As…", and "Delete Workspace: `name`"
/// for each user preset (never Essentials).
pub(crate) fn workspace_palette_entries(presets: &WorkspacePresets) -> Vec<CommandEntry> {
    let switch = |name: &str| {
        CommandEntry::new(
            format!("{COMMAND_WORKSPACE_SWITCH_PREFIX}{name}"),
            format!("Workspace: {name}"),
        )
    };
    let mut entries = vec![switch(ESSENTIALS)];
    entries.extend(presets.user().iter().map(|preset| switch(&preset.name)));
    entries.push(CommandEntry::new(
        COMMAND_WORKSPACE_RESET_ACTIVE,
        format!("Reset {}", presets.active_name()),
    ));
    entries.push(CommandEntry::new(
        COMMAND_WORKSPACE_SAVE_AS,
        "Save Workspace As\u{2026}",
    ));
    entries.extend(presets.user().iter().map(|preset| {
        CommandEntry::new(
            format!("{COMMAND_WORKSPACE_DELETE_PREFIX}{}", preset.name),
            format!("Delete Workspace: {}", preset.name),
        )
    }));
    entries
}

/// Adds the workspace entries to a palette the key just opened (the
/// palette's static list is [`super::palette_commands`]; these depend on
/// the presets, so they are appended once it is open). A no-op when the
/// palette was already open, is closed, or is the name prompt.
pub(crate) fn add_workspace_entries_if_opened(
    workspace: &mut aurora_ui::Workspace,
    was_open: bool,
    palette: Option<WidgetId>,
    presets: &WorkspacePresets,
) -> bool {
    let Some(root) = palette else {
        return false;
    };
    if was_open {
        return false;
    }
    let Ok(state) = command_palette_state(&workspace.tree, root) else {
        return false;
    };
    if !state.is_filtering() {
        return false;
    }
    let mut commands = state.commands().to_vec();
    commands.extend(workspace_palette_entries(presets));
    if let Err(err) = set_command_palette_commands(&mut workspace.tree, root, commands) {
        tracing::warn!(?err, "failed to add the workspace commands to the palette");
        return false;
    }
    true
}

/// The name prompt's one row for `query`: what Enter does.
fn name_prompt_entries(query: &str, replace: bool) -> Vec<CommandEntry> {
    let name = query.trim();
    if replace {
        return vec![CommandEntry::new(
            COMMAND_WORKSPACE_NAME_REPLACE,
            format!("Replace Workspace \u{201c}{name}\u{201d}"),
        )];
    }
    let title = if name.is_empty() {
        "Save Workspace".to_owned()
    } else {
        format!("Save Workspace \u{201c}{name}\u{201d}")
    };
    vec![CommandEntry::new(COMMAND_WORKSPACE_NAME_SAVE, title)]
}

/// Whether `id` is one of the name prompt's rows.
fn is_name_prompt_row(id: &str) -> bool {
    id == COMMAND_WORKSPACE_NAME_SAVE || id == COMMAND_WORKSPACE_NAME_REPLACE
}

/// Opens the name prompt (a no-op if a palette or prompt is open): the
/// typed text starts as `query`; with `replace`, its row and message say
/// the name is taken and that Enter replaces it. Focus moves to it.
/// Returns whether it opened.
pub(crate) fn open_workspace_name_prompt(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
    query: &str,
    replace: bool,
) -> bool {
    if palette.is_some() {
        return false;
    }
    let root = match insert_command_prompt(
        &mut workspace.tree,
        workspace.root,
        WORKSPACE_NAME_PROMPT_LABEL,
        name_prompt_entries(query, replace),
    ) {
        Ok(root) => root,
        Err(err) => {
            tracing::warn!(?err, "failed to open the workspace name prompt");
            return false;
        }
    };
    if let Err(err) = workspace.tree.set_style(root, command_palette_style()) {
        tracing::warn!(?err, "failed to size the workspace name prompt");
    }
    if !query.is_empty()
        && let Err(err) = set_command_palette_query(&mut workspace.tree, root, query)
    {
        tracing::warn!(?err, "failed to fill the workspace name prompt");
    }
    if replace {
        let message = format!(
            "A workspace named \u{201c}{}\u{201d} exists. Press Enter to replace it, \
             or Escape to cancel.",
            query.trim()
        );
        if let Err(err) = set_command_palette_message(&mut workspace.tree, root, Some(message)) {
            tracing::warn!(?err, "failed to describe the workspace name prompt");
        }
    }
    if let Err(err) = focus.focus(&mut workspace.tree, root) {
        tracing::warn!(?err, "failed to focus the workspace name prompt");
    }
    *palette = Some(root);
    true
}

/// After the typed text changed: a name prompt's row follows it (and a
/// replace confirmation or a refusal is withdrawn — an edited name is
/// checked afresh). A no-op for the ordinary palette.
pub(crate) fn follow_name_prompt_query(
    tree: &mut aurora_widgets::WidgetTree<super::WidgetKind>,
    root: WidgetId,
) {
    let Ok(state) = command_palette_state(tree, root) else {
        return;
    };
    if state.is_filtering() {
        return;
    }
    let query = state.query().to_owned();
    if let Err(err) = set_command_palette_commands(tree, root, name_prompt_entries(&query, false)) {
        tracing::warn!(?err, "failed to update the workspace name prompt");
    }
    if let Err(err) = set_command_palette_message(tree, root, None) {
        tracing::warn!(?err, "failed to clear the workspace name prompt's message");
    }
}

/// The name prompt's Enter. `None` when `root` is no name prompt (the
/// caller activates the selected command as usual). Otherwise
/// `Some(Ok(command))` for a valid name — the caller closes the prompt and
/// runs it — or `Some(Err(()))` for a refused one: the prompt stays open,
/// its row shows the reason and its accessible description carries it.
pub(crate) fn name_prompt_enter(
    tree: &mut aurora_widgets::WidgetTree<super::WidgetKind>,
    root: WidgetId,
) -> Option<Result<WorkspaceCommand, ()>> {
    let state = command_palette_state(tree, root).ok()?;
    if state.is_filtering() {
        return None;
    }
    let id = state.selected().map(|entry| entry.id.clone())?;
    if !is_name_prompt_row(&id) {
        return None;
    }
    let query = state.query().to_owned();
    match validate_workspace_name(&query) {
        Ok(name) => Some(Ok(WorkspaceCommand::Save {
            name,
            replace: id == COMMAND_WORKSPACE_NAME_REPLACE,
        })),
        Err(err) => {
            let message = err.message();
            let row = vec![CommandEntry::new(
                COMMAND_WORKSPACE_NAME_SAVE,
                message.clone(),
            )];
            if let Err(err) = set_command_palette_commands(tree, root, row) {
                tracing::warn!(?err, "failed to show a refused workspace name");
            }
            if let Err(err) = set_command_palette_message(tree, root, Some(message)) {
                tracing::warn!(?err, "failed to describe a refused workspace name");
            }
            Some(Err(()))
        }
    }
}

/// The panels whose body is empty (0.168.0 review J5): a close empties a
/// body (`aurora_ui::close_panel`) and a preset's apply only expands it
/// again, so after an apply the caller refills each of these
/// (`super::refill_reopened_panels`). Read off the bodies rather than
/// `aurora_ui::panel_is_closed`, which cannot tell a closed *grouped*
/// panel from an open one (a group member's title row is always hidden).
/// A body that is legitimately empty (Properties for a tool with no
/// options) is refilled to the same empty state, harmlessly.
pub(crate) fn emptied_panels(workspace: &aurora_ui::Workspace) -> Vec<aurora_ui::DockPanel> {
    aurora_ui::DockPanel::ALL
        .into_iter()
        .filter(|&panel| {
            workspace
                .tree
                .children(workspace.panel(panel).body)
                .is_none_or(<[_]>::is_empty)
        })
        .collect()
}

/// What [`run_workspace_command`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct WorkspaceCommandOutcome {
    /// The live layout was re-applied (the caller lays out again).
    pub(crate) applied: bool,
    /// The presets or the active name changed (the caller saves them).
    pub(crate) presets_changed: bool,
}

/// Runs one workspace command against the live workspace and the presets.
/// Never touches document state or `History`.
pub(crate) fn run_workspace_command(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
    presets: &mut WorkspacePresets,
    scales: &Scales,
    command: WorkspaceCommand,
) -> WorkspaceCommandOutcome {
    match command {
        WorkspaceCommand::Switch(name) => {
            let Some(layout) = presets.layout_of(&name) else {
                tracing::warn!(name, "no such workspace");
                return WorkspaceCommandOutcome::default();
            };
            apply_layout_keeping_focus(workspace, focus, &layout, scales);
            let before = presets.active.clone();
            presets.set_active(&name);
            WorkspaceCommandOutcome {
                applied: true,
                presets_changed: presets.active != before,
            }
        }
        WorkspaceCommand::ResetActive => {
            let layout = presets
                .layout_of(presets.active_name())
                .unwrap_or_else(essentials_layout);
            apply_layout_keeping_focus(workspace, focus, &layout, scales);
            WorkspaceCommandOutcome {
                applied: true,
                presets_changed: false,
            }
        }
        WorkspaceCommand::Delete(name) => WorkspaceCommandOutcome {
            applied: false,
            presets_changed: presets.delete(&name),
        },
        WorkspaceCommand::PromptSaveAs => {
            open_workspace_name_prompt(workspace, focus, palette, "", false);
            WorkspaceCommandOutcome::default()
        }
        WorkspaceCommand::Save { name, replace } => {
            let Ok(name) = validate_workspace_name(&name) else {
                return WorkspaceCommandOutcome::default();
            };
            if !replace && presets.has_user(&name) {
                // Taken: ask again, in the prompt itself.
                open_workspace_name_prompt(workspace, focus, palette, &name, true);
                return WorkspaceCommandOutcome::default();
            }
            if !presets.has_user(&name) && presets.user.len() >= MAX_WORKSPACE_PRESETS {
                // Review J2: full — say so in the prompt, save nothing.
                if open_workspace_name_prompt(workspace, focus, palette, &name, false)
                    && let Some(root) = *palette
                {
                    let message = format!(
                        "At most {MAX_WORKSPACE_PRESETS} workspaces can be saved; delete one, \
                         or save over an existing name."
                    );
                    if let Err(err) =
                        set_command_palette_message(&mut workspace.tree, root, Some(message))
                    {
                        tracing::warn!(?err, "failed to describe a full workspace list");
                    }
                }
                return WorkspaceCommandOutcome::default();
            }
            presets.save(name, live_layout(workspace));
            WorkspaceCommandOutcome {
                applied: false,
                presets_changed: true,
            }
        }
    }
}

// -- The presets file --

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PresetsHeader {
    magic: [u8; 4],
    version: u32,
    /// The active preset's name; empty for Essentials.
    active: String,
    count: u32,
}

/// One preset on disk: its name and its layout's own `postcard` bytes,
/// decoded with the live layout's fallback chain, so a damaged layout
/// costs only its own preset.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct SavedPreset {
    name: String,
    layout: Vec<u8>,
}

/// `layout_path`'s sibling presets file.
pub(crate) fn presets_path_for(layout_path: &Path) -> PathBuf {
    layout_path.with_file_name(PRESETS_FILE_NAME)
}

/// The presets as bytes: a header, then each preset in turn.
pub(crate) fn encode_presets(presets: &WorkspacePresets) -> Result<Vec<u8>, postcard::Error> {
    let header = PresetsHeader {
        magic: PRESETS_MAGIC,
        version: PRESETS_VERSION,
        active: presets.active.clone().unwrap_or_default(),
        count: u32::try_from(presets.user.len()).unwrap_or(u32::MAX),
    };
    let mut bytes = postcard::to_allocvec(&header)?;
    for preset in &presets.user {
        let entry = SavedPreset {
            name: preset.name.clone(),
            layout: postcard::to_allocvec(&preset.layout)?,
        };
        bytes.extend(postcard::to_allocvec(&entry)?);
    }
    Ok(bytes)
}

/// A decoded layout made valid: a non-finite rail width becomes the
/// default (`set_rail_width` clamps a finite one), and the arrangement is
/// repaired (`arrangement_from_saved`, so every panel is placed once and
/// the float stack is consistent). Returns whether anything was repaired.
fn repaired_layout(mut layout: WorkspaceLayout) -> (WorkspaceLayout, bool) {
    let mut repaired = false;
    if !layout.rail_width.is_finite() {
        layout.rail_width = aurora_ui::RAIL_WIDTH_DEFAULT;
        repaired = true;
    }
    let (arrangement, arrangement_repaired) =
        arrangement_from_saved(&layout.dock, &layout.float_stack);
    layout.dock = saved_dock(&arrangement);
    layout.float_stack = saved_float_stack(&arrangement);
    (layout, repaired || arrangement_repaired)
}

/// Decodes a presets file, never failing: a bad header (garbage, a
/// truncated file, another version) gives no user presets and Essentials
/// active; a preset that fails to decode, has an invalid name or repeats
/// an earlier name (case-insensitive) is dropped, and decoding stops at
/// the first truncated one, keeping those before it; each kept layout is
/// repaired ([`repaired_layout`]); an active name naming no kept preset
/// falls back to Essentials. Returns whether anything was dropped or
/// repaired.
pub(crate) fn decode_presets(bytes: &[u8]) -> (WorkspacePresets, bool) {
    let Ok((header, mut rest)) = postcard::take_from_bytes::<PresetsHeader>(bytes) else {
        return (WorkspacePresets::default(), !bytes.is_empty());
    };
    if header.magic != PRESETS_MAGIC || header.version != PRESETS_VERSION {
        return (WorkspacePresets::default(), true);
    }
    let mut presets = WorkspacePresets::default();
    let mut repaired = false;
    for index in 0..header.count {
        // Review J2: at most `MAX_WORKSPACE_PRESETS` entries are read.
        // (`has_user` lowercases every kept name per check, so this loop
        // is quadratic in the kept count — bounded by the cap.)
        if usize::try_from(index).map_or(true, |index| index >= MAX_WORKSPACE_PRESETS) {
            repaired = true;
            break;
        }
        let Ok((entry, after)) = postcard::take_from_bytes::<SavedPreset>(rest) else {
            repaired = true;
            break;
        };
        rest = after;
        let Ok(name) = validate_workspace_name(&entry.name) else {
            repaired = true;
            continue;
        };
        if presets.has_user(&name) {
            repaired = true;
            continue;
        }
        repaired |= name != entry.name;
        let Ok(layout) = decode_workspace_layout(&entry.layout) else {
            repaired = true;
            continue;
        };
        let (layout, layout_repaired) = repaired_layout(layout);
        repaired |= layout_repaired;
        presets.user.push(WorkspacePreset { name, layout });
    }
    if !header.active.is_empty() && !presets.set_active(&header.active) {
        repaired = true;
    }
    (presets, repaired)
}

/// Reads the presets at `path`; a missing, unreadable or damaged file
/// never fails — see [`decode_presets`].
pub(crate) fn load_presets(path: &Path) -> WorkspacePresets {
    let bytes = match read_capped(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(?err, path = %path.display(), "failed to read the workspace presets");
            }
            return WorkspacePresets::default();
        }
    };
    let (presets, repaired) = decode_presets(&bytes);
    if repaired {
        tracing::warn!(path = %path.display(), "the workspace presets were damaged; repaired");
    }
    presets
}

/// Reads at most [`PRESETS_FILE_MAX_BYTES`] of `path` (review J2): a
/// larger file is decoded from that prefix alone, which keeps every whole
/// entry in it and drops the truncated one.
fn read_capped(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(PRESETS_FILE_MAX_BYTES)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Writes the presets to `path` safely: to a temporary file beside it,
/// synced, then renamed over `path`, so a failed write never leaves a
/// half-written file where the old one was.
pub(crate) fn save_presets(path: &Path, presets: &WorkspacePresets) -> std::io::Result<()> {
    let bytes = encode_presets(presets).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Review J3: the process id in the name, so two Aurora processes never
    // write the same temporary file, and the temporary removed on failure.
    let temporary = temporary_path(path);
    let written = write_synced(&temporary, &bytes).and_then(|()| std::fs::rename(&temporary, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// The temporary file [`save_presets`] writes before renaming: beside
/// `path`, named after it and this process's id.
fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| PRESETS_FILE_NAME.into(), std::ffi::OsStr::to_os_string);
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::{
        COMMAND_WORKSPACE_NAME_REPLACE, COMMAND_WORKSPACE_NAME_SAVE, COMMAND_WORKSPACE_SAVE_AS,
        ESSENTIALS, MAX_WORKSPACE_PRESETS, PRESETS_FILE_MAX_BYTES, PRESETS_MAGIC, PresetsHeader,
        SavedPreset, WORKSPACE_NAME_MAX_CHARS, WORKSPACE_NAME_PROMPT_LABEL, WorkspaceCommand,
        WorkspaceCommandOutcome, WorkspaceNameError, WorkspacePresets,
        add_workspace_entries_if_opened, decode_presets, encode_presets, essentials_layout,
        live_layout, load_presets, presets_path_for, run_workspace_command, save_presets,
        validate_workspace_name, workspace_command_for, workspace_palette_entries,
    };
    use crate::tests::{FakeClipboard, FakeFileDialog};
    use crate::{
        ActivatedCommand, COMMAND_RESET_PANELS, FocusManager, Key, KeyChord, Modifiers, NamedKey,
        SavedPlacement, WidgetId, WorkspaceLayoutV1, WorkspaceLayoutV4, activate_command,
        handle_palette_key, run_panel_float, toggle_command_palette,
    };
    use aurora_ui::DockPanel;
    use aurora_widgets::widgets::command_palette_state;

    const WINDOW: (f32, f32) = (1000.0, 800.0);

    fn lay(ws: &mut aurora_ui::Workspace, scale: f64, size: (f32, f32)) {
        crate::layout_workspace(
            ws,
            None,
            &crate::test_layout_scales(),
            scale,
            size.0,
            size.1,
        );
    }

    fn workspace() -> aurora_ui::Workspace {
        let mut ws = aurora_ui::build_workspace(&crate::test_workspace_scales());
        lay(&mut ws, 1.0, WINDOW);
        ws
    }

    struct Rig {
        ws: aurora_ui::Workspace,
        focus: FocusManager,
        palette: Option<WidgetId>,
        presets: WorkspacePresets,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                ws: workspace(),
                focus: FocusManager::default(),
                palette: None,
                presets: WorkspacePresets::default(),
            }
        }

        fn run(&mut self, command: WorkspaceCommand) -> WorkspaceCommandOutcome {
            let outcome = run_workspace_command(
                &mut self.ws,
                &mut self.focus,
                &mut self.palette,
                &mut self.presets,
                &crate::test_layout_scales(),
                command,
            );
            lay(&mut self.ws, 1.0, WINDOW);
            outcome
        }

        fn key(&mut self, key: Key, text: Option<&str>) -> Option<ActivatedCommand> {
            let picked = handle_palette_key(
                &mut self.ws,
                &mut self.focus,
                &mut self.palette,
                KeyChord::new(Modifiers::none(), key),
                text,
                &mut FakeClipboard::default(),
                &mut FakeFileDialog::default(),
            );
            lay(&mut self.ws, 1.0, WINDOW);
            picked
        }

        fn type_text(&mut self, text: &str) {
            for c in text.chars() {
                let key = if c == ' ' {
                    Key::Named(NamedKey::Space)
                } else {
                    Key::Character(c)
                };
                let typed = c.to_string();
                assert_eq!(self.key(key, Some(&typed)), None);
            }
        }

        /// Opens the prompt through the palette command, types `name` and
        /// presses Enter; returns what Enter handed back.
        fn save_as(&mut self, name: &str) -> Option<ActivatedCommand> {
            let picked = activate_command(
                &mut self.ws,
                &mut self.focus,
                COMMAND_WORKSPACE_SAVE_AS,
                &mut FakeFileDialog::default(),
            );
            assert_eq!(
                picked,
                Some(ActivatedCommand::Workspace(WorkspaceCommand::PromptSaveAs))
            );
            self.run(WorkspaceCommand::PromptSaveAs);
            self.type_text(name);
            self.key(Key::Named(NamedKey::Enter), None)
        }

        /// Saves `name` all the way (the prompt's Enter, then the command).
        fn save(&mut self, name: &str) {
            match self.save_as(name) {
                Some(ActivatedCommand::Workspace(command)) => {
                    let outcome = self.run(command);
                    assert!(outcome.presets_changed, "{name} saved");
                }
                other => unreachable!("{name}: {other:?}"),
            }
        }

        fn prompt(&self) -> (String, Option<String>, Vec<(String, String)>) {
            let Some(root) = self.palette else {
                unreachable!("the prompt is open");
            };
            let state = match command_palette_state(&self.ws.tree, root) {
                Ok(state) => state,
                Err(err) => unreachable!("{err:?}"),
            };
            let node = self.ws.tree.accessibility(root).cloned();
            assert_eq!(
                node.as_ref().map(accesskit::Node::role),
                Some(accesskit::Role::TextInput)
            );
            (
                node.as_ref()
                    .and_then(|n| n.label())
                    .unwrap_or_default()
                    .to_owned(),
                node.as_ref()
                    .and_then(|n| n.description())
                    .map(str::to_owned),
                state
                    .results()
                    .iter()
                    .map(|e| (e.id.clone(), e.title.clone()))
                    .collect(),
            )
        }

        fn palette_titles(&mut self) -> Vec<String> {
            let mut palette = None;
            toggle_command_palette(&mut self.ws, &mut self.focus, &mut palette);
            assert!(add_workspace_entries_if_opened(
                &mut self.ws,
                false,
                palette,
                &self.presets
            ));
            let titles = palette
                .and_then(|root| command_palette_state(&self.ws.tree, root).ok())
                .map(|state| {
                    state
                        .results()
                        .iter()
                        .map(|e| e.title.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            toggle_command_palette(&mut self.ws, &mut self.focus, &mut palette);
            titles
        }
    }

    fn set_width(ws: &mut aurora_ui::Workspace, width: f32) {
        if let Err(err) = aurora_ui::set_rail_width(&mut ws.tree, ws.rail, ws.divider, width) {
            unreachable!("{err:?}");
        }
    }

    /// Changes every part of the live layout a preset captures.
    fn edit_everything(rig: &mut Rig) {
        let scales = crate::test_layout_scales();
        set_width(&mut rig.ws, 410.0);
        assert!(run_panel_float(
            &mut rig.ws,
            &mut rig.focus,
            &scales,
            DockPanel::Layers,
            true
        ));
        let history = rig.ws.history;
        if let Err(err) = aurora_ui::select_panel_tab(&mut rig.ws, history) {
            unreachable!("{err:?}");
        }
        if let Err(err) = aurora_ui::set_rail_collapsed(&mut rig.ws, true) {
            unreachable!("{err:?}");
        }
        lay(&mut rig.ws, 1.0, WINDOW);
    }

    /// AC-1: Essentials is the default layout, and switching to it (or
    /// Reset Panel Layout, its alias) from a changed one restores it.
    #[test]
    fn essentials_is_the_default_layout_and_switching_to_it_restores_it() {
        let mut rig = Rig::new();
        assert_eq!(live_layout(&rig.ws), essentials_layout());
        assert_eq!(rig.presets.active_name(), ESSENTIALS);
        edit_everything(&mut rig);
        assert_ne!(live_layout(&rig.ws), essentials_layout());
        let outcome = rig.run(WorkspaceCommand::Switch("essentials".to_owned()));
        assert!(outcome.applied);
        assert_eq!(live_layout(&rig.ws), essentials_layout());
        assert!(rig.ws.floating.is_empty());
        assert!(!aurora_ui::rail_collapsed(&rig.ws));
        assert_eq!(
            rig.ws.dock_arrangement(),
            aurora_ui::DockArrangement::default()
        );

        // Reset Panel Layout is the same switch, and makes Essentials active.
        rig.save("Paint");
        assert_eq!(rig.presets.active_name(), "Paint");
        edit_everything(&mut rig);
        assert_eq!(
            activate_command(
                &mut rig.ws,
                &mut rig.focus,
                COMMAND_RESET_PANELS,
                &mut FakeFileDialog::default()
            ),
            Some(ActivatedCommand::ResetPanels)
        );
        let outcome = rig.run(WorkspaceCommand::Switch(ESSENTIALS.to_owned()));
        assert!(outcome.presets_changed, "the active name changed");
        assert_eq!(rig.presets.active_name(), ESSENTIALS);
        assert_eq!(live_layout(&rig.ws), essentials_layout());
        assert!(
            crate::reset_panel_arrangement(
                &mut rig.ws,
                &mut rig.focus,
                &crate::test_layout_scales()
            ) || live_layout(&rig.ws) == essentials_layout()
        );
    }

    /// AC-2 (the rules): trimmed, non-empty, capped, never Essentials.
    #[test]
    fn workspace_names_are_trimmed_capped_and_never_essentials() {
        assert_eq!(validate_workspace_name("  Paint  "), Ok("Paint".to_owned()));
        assert_eq!(validate_workspace_name(""), Err(WorkspaceNameError::Empty));
        assert_eq!(
            validate_workspace_name(" \t "),
            Err(WorkspaceNameError::Empty)
        );
        let longest = "é".repeat(WORKSPACE_NAME_MAX_CHARS);
        assert_eq!(validate_workspace_name(&longest), Ok(longest.clone()));
        assert_eq!(
            validate_workspace_name(&format!("{longest}x")),
            Err(WorkspaceNameError::TooLong)
        );
        for reserved in ["Essentials", "ESSENTIALS", " essentials ", "eSsEnTiAlS"] {
            assert_eq!(
                validate_workspace_name(reserved),
                Err(WorkspaceNameError::Reserved),
                "{reserved:?}"
            );
        }
        assert_eq!(
            validate_workspace_name("Essentials 2"),
            Ok("Essentials 2".to_owned())
        );
    }

    /// Review J1: control, separator and bidi override/isolate characters
    /// are refused — typed, and read from a file.
    #[test]
    fn workspace_names_with_control_or_bidi_characters_are_refused() {
        for bad in [
            "Paint\u{202e}gnp",
            "a\u{202a}b",
            "a\u{2066}b",
            "a\u{2069}b",
            "a\u{0007}b",
            "a\u{0000}b",
            "a\nb",
            "a\u{2028}b",
            "\u{202e}",
        ] {
            assert_eq!(
                validate_workspace_name(bad),
                Err(WorkspaceNameError::InvalidCharacter),
                "{bad:?}"
            );
        }
        assert_eq!(
            validate_workspace_name("  Ébauche 2 ✎ "),
            Ok("Ébauche 2 ✎".to_owned())
        );
        // Typed into the prompt: refused with the accessible message.
        let mut rig = Rig::new();
        assert_eq!(rig.save_as("Paint\u{202e}x"), None);
        let (_, description, _) = rig.prompt();
        assert_eq!(
            description,
            Some(WorkspaceNameError::InvalidCharacter.message())
        );
        // Read from a file: dropped.
        let essentials = encode(&essentials_layout());
        let bytes = file(
            "",
            &[
                ("Ev\u{202e}il".to_owned(), essentials.clone()),
                ("Good".to_owned(), essentials),
            ],
        );
        let (decoded, repaired) = decode_presets(&bytes);
        assert!(repaired);
        let names: Vec<&str> = decoded.user().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Good"]);
    }

    /// Review J2: decoding stops at `MAX_WORKSPACE_PRESETS` entries and
    /// reads at most `PRESETS_FILE_MAX_BYTES`; Save As refuses a new name
    /// once the list is full (an existing name can still be replaced).
    #[test]
    fn the_presets_file_and_list_are_capped() {
        let essentials = encode(&essentials_layout());
        let many: Vec<(String, Vec<u8>)> = (0..MAX_WORKSPACE_PRESETS + 5)
            .map(|i| (format!("P{i}"), essentials.clone()))
            .collect();
        let (decoded, repaired) = decode_presets(&file("", &many));
        assert!(repaired);
        assert_eq!(decoded.user().len(), MAX_WORKSPACE_PRESETS);
        let exact = many.get(..MAX_WORKSPACE_PRESETS).unwrap_or_default();
        let (decoded, repaired) = decode_presets(&file("", exact));
        assert!(!repaired);
        assert_eq!(decoded.user().len(), MAX_WORKSPACE_PRESETS);

        // A file past the byte cap is decoded from its capped prefix.
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("workspace-presets.postcard");
        let mut big = file("", &[("Kept".to_owned(), essentials)]);
        let cap = usize::try_from(PRESETS_FILE_MAX_BYTES).unwrap_or(usize::MAX);
        big.resize(cap * 2, 0xff);
        if let Err(err) = std::fs::write(&path, &big) {
            unreachable!("{err}");
        }
        assert_eq!(super::read_capped(&path).map(|b| b.len()).ok(), Some(cap));
        let loaded = load_presets(&path);
        assert_eq!(
            loaded.user().len(),
            1,
            "the whole entry inside the cap is kept"
        );

        // Save As with the list full.
        let mut rig = Rig::new();
        rig.presets = decoded;
        let outcome = rig.run(WorkspaceCommand::Save {
            name: "One Too Many".to_owned(),
            replace: false,
        });
        assert!(!outcome.presets_changed);
        assert_eq!(rig.presets.user().len(), MAX_WORKSPACE_PRESETS);
        let (_, description, _) = rig.prompt();
        assert!(description.is_some_and(|d| d.contains("At most")));
        assert_eq!(rig.key(Key::Named(NamedKey::Escape), None), None);
        let outcome = rig.run(WorkspaceCommand::Save {
            name: "P3".to_owned(),
            replace: true,
        });
        assert!(outcome.presets_changed, "an existing name still saves");
    }

    /// Review J3: the temporary file carries the process id and is removed
    /// when the write or the rename fails.
    #[test]
    fn a_failed_save_leaves_no_temporary_file() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("workspace-presets.postcard");
        let temporary = super::temporary_path(&path);
        let name = temporary
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        assert!(name.contains(&std::process::id().to_string()), "{name}");
        assert_eq!(temporary.parent(), path.parent());
        // The rename fails: a non-empty directory sits where the file goes.
        if let Err(err) = std::fs::create_dir_all(path.join("occupied")) {
            unreachable!("{err}");
        }
        assert!(save_presets(&path, &WorkspacePresets::default()).is_err());
        assert!(!temporary.exists(), "removed after a failed rename");
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.file_name())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(left.len(), 1, "only the blocking directory: {left:?}");
    }

    /// Review J5: a preset switch that reopens closed panels refills their
    /// bodies (Layers rows, History rows, Properties options).
    #[test]
    fn a_switch_that_reopens_closed_panels_refills_them() {
        let mut rig = Rig::new();
        let scales = crate::test_layout_scales();
        let (layers, _history) = crate::demo_document();
        let undo_order = crate::UndoOrder::default();
        let tool = aurora_ui::Tool::Brush;
        let settings = crate::ToolSettings::default();
        let panels = crate::install_startup_panels(
            &mut rig.ws,
            &scales,
            &layers,
            &undo_order,
            tool,
            &settings,
        );
        let mut layer_rows = panels.layer_rows;
        let mut active_layer = panels.active_layer;
        let mut view = aurora_ui::CanvasView::default();
        let body_len = |ws: &aurora_ui::Workspace, panel: DockPanel| {
            ws.tree.children(ws.panel(panel).body).map_or(0, <[_]>::len)
        };
        for panel in DockPanel::ALL {
            assert!(body_len(&rig.ws, panel) > 0, "{panel:?} populated");
            let handle = rig.ws.panel(panel);
            if let Err(err) = aurora_ui::close_workspace_panel(&mut rig.ws, handle) {
                unreachable!("{err:?}");
            }
            assert_eq!(
                body_len(&rig.ws, panel),
                0,
                "{panel:?} emptied by the close"
            );
        }
        rig.run(WorkspaceCommand::Switch(ESSENTIALS.to_owned()));
        let reopened = super::emptied_panels(&rig.ws);
        assert_eq!(
            reopened,
            DockPanel::ALL.to_vec(),
            "every closed body is empty"
        );
        crate::refill_reopened_panels(
            &mut rig.ws,
            &mut rig.focus,
            &scales,
            &reopened,
            &crate::RefillDocument {
                layers: &layers,
                undo_order: &undo_order,
                tool,
                tool_settings: &settings,
            },
            &mut layer_rows,
            &mut active_layer,
            &mut view,
        );
        for panel in DockPanel::ALL {
            assert!(body_len(&rig.ws, panel) > 0, "{panel:?} refilled");
        }
        assert!(!layer_rows.is_empty());
        assert!(layer_rows.keys().all(|row| rig.ws.tree.contains(*row)));
        // Nothing emptied: nothing to refill.
        rig.run(WorkspaceCommand::ResetActive);
        assert!(super::emptied_panels(&rig.ws).is_empty());
    }

    /// AC-2: Save As captures the full layout and makes it active; invalid
    /// names keep the prompt open with the reason as its row and its
    /// accessible description; saving over a taken name asks once, in the
    /// prompt, then overwrites.
    #[test]
    fn save_as_captures_the_layout_refuses_bad_names_and_confirms_overwrites() {
        let mut rig = Rig::new();
        edit_everything(&mut rig);
        let captured = live_layout(&rig.ws);
        rig.save("  Paint ");
        assert_eq!(rig.presets.active_name(), "Paint");
        assert_eq!(rig.presets.layout_of("paint"), Some(captured.clone()));
        assert!((captured.rail_width - 410.0).abs() < f32::EPSILON);
        assert!(captured.rail_collapsed);
        assert_eq!(captured.panel_group_tab, 1, "History's tab");
        assert!(
            captured
                .dock
                .iter()
                .any(|slot| matches!(slot.placement, SavedPlacement::Floating { .. })),
            "the float"
        );
        assert_eq!(captured.float_stack.len(), 1);

        let too_long = "x".repeat(WORKSPACE_NAME_MAX_CHARS + 1);
        for (typed, error) in [
            ("", WorkspaceNameError::Empty),
            ("   ", WorkspaceNameError::Empty),
            (too_long.as_str(), WorkspaceNameError::TooLong),
            ("Essentials", WorkspaceNameError::Reserved),
            ("ESSENTIALS", WorkspaceNameError::Reserved),
        ] {
            assert_eq!(rig.save_as(typed), None, "{typed:?} refused");
            let (label, description, rows) = rig.prompt();
            assert_eq!(label, WORKSPACE_NAME_PROMPT_LABEL);
            assert_eq!(description, Some(error.message()), "{typed:?}");
            assert_eq!(
                rows,
                vec![(COMMAND_WORKSPACE_NAME_SAVE.to_owned(), error.message())]
            );
            assert_eq!(
                rig.focus.focused(),
                rig.palette,
                "focus stays in the prompt"
            );
            // Editing withdraws the refusal.
            rig.type_text("a");
            let (_, description, _) = rig.prompt();
            assert_eq!(description, None);
            assert_eq!(rig.key(Key::Named(NamedKey::Escape), None), None);
            assert!(rig.palette.is_none());
        }
        assert_eq!(rig.presets.user().len(), 1, "nothing saved");

        // Overwrite: the first Enter on a taken name asks, the second replaces.
        if let Err(err) = aurora_ui::set_rail_collapsed(&mut rig.ws, false) {
            unreachable!("{err:?}");
        }
        set_width(&mut rig.ws, 333.0);
        lay(&mut rig.ws, 1.0, WINDOW);
        let picked = rig.save_as("PAINT");
        assert_eq!(
            picked,
            Some(ActivatedCommand::Workspace(WorkspaceCommand::Save {
                name: "PAINT".to_owned(),
                replace: false
            }))
        );
        let Some(ActivatedCommand::Workspace(command)) = picked else {
            unreachable!();
        };
        let outcome = rig.run(command);
        assert!(!outcome.presets_changed, "not yet");
        assert_eq!(rig.presets.layout_of("Paint"), Some(captured.clone()));
        let (_, description, rows) = rig.prompt();
        assert!(description.is_some_and(|d| d.contains("exists")));
        assert_eq!(
            rows.first().map(|r| r.0.as_str()),
            Some(COMMAND_WORKSPACE_NAME_REPLACE)
        );
        assert_eq!(rig.focus.focused(), rig.palette);
        let picked = rig.key(Key::Named(NamedKey::Enter), None);
        let Some(ActivatedCommand::Workspace(command)) = picked else {
            unreachable!("{picked:?}");
        };
        assert!(rig.run(command).presets_changed);
        assert_eq!(rig.presets.user().len(), 1, "overwritten, not added");
        assert_eq!(rig.presets.active_name(), "PAINT", "the new spelling");
        assert_eq!(rig.presets.layout_of("paint"), Some(live_layout(&rig.ws)));
        assert_ne!(rig.presets.layout_of("paint"), Some(captured));
    }

    /// AC-3: the palette's workspace entries appear and disappear with
    /// saves and deletes, and each resolves to its command.
    #[test]
    fn palette_workspace_entries_follow_saves_and_deletes() {
        let mut rig = Rig::new();
        let titles = rig.palette_titles();
        for wanted in [
            "Workspace: Essentials",
            "Reset Essentials",
            "Save Workspace As\u{2026}",
            "Focus Layers Panel",
        ] {
            assert!(titles.iter().any(|t| t == wanted), "{wanted} in {titles:?}");
        }
        assert!(!titles.iter().any(|t| t.starts_with("Delete Workspace")));
        rig.save("Paint");
        rig.save("Ink");
        let titles = rig.palette_titles();
        for wanted in [
            "Workspace: Paint",
            "Workspace: Ink",
            "Delete Workspace: Paint",
            "Delete Workspace: Ink",
            "Reset Ink",
        ] {
            assert!(titles.iter().any(|t| t == wanted), "{wanted} in {titles:?}");
        }
        assert!(!titles.iter().any(|t| t == "Delete Workspace: Essentials"));
        for entry in workspace_palette_entries(&rig.presets) {
            let resolved = activate_command(
                &mut rig.ws,
                &mut rig.focus,
                &entry.id,
                &mut FakeFileDialog::default(),
            );
            assert!(
                matches!(resolved, Some(ActivatedCommand::Workspace(_))),
                "{} resolves",
                entry.id
            );
        }
        assert_eq!(
            workspace_command_for("workspace.switch/Paint"),
            Some(WorkspaceCommand::Switch("Paint".to_owned()))
        );
        rig.run(WorkspaceCommand::Delete("Paint".to_owned()));
        let titles = rig.palette_titles();
        assert!(!titles.iter().any(|t| t.contains("Paint")), "{titles:?}");
        assert!(titles.iter().any(|t| t == "Workspace: Ink"));
        // Only a palette the key just opened is extended, never the prompt.
        let mut palette = None;
        toggle_command_palette(&mut rig.ws, &mut rig.focus, &mut palette);
        assert!(!add_workspace_entries_if_opened(
            &mut rig.ws,
            true,
            palette,
            &rig.presets
        ));
        toggle_command_palette(&mut rig.ws, &mut rig.focus, &mut palette);
        rig.run(WorkspaceCommand::PromptSaveAs);
        assert!(!add_workspace_entries_if_opened(
            &mut rig.ws,
            false,
            rig.palette,
            &rig.presets
        ));
    }

    /// AC-3: switching keeps focus on the panel it held, moves it off a
    /// rail the preset collapses, and clamps the preset's floats into the
    /// window, at scale 1 and 2.
    #[test]
    fn switching_keeps_focus_and_clamps_floats_at_scale_one_and_two() {
        for scale in [1.0_f64, 2.0] {
            #[allow(clippy::cast_possible_truncation)]
            let logical = (1200.0 / scale as f32, 900.0 / scale as f32);
            let mut rig = Rig::new();
            let scales = crate::test_layout_scales();
            assert!(run_panel_float(
                &mut rig.ws,
                &mut rig.focus,
                &scales,
                DockPanel::Layers,
                true
            ));
            rig.save("Far");
            // The saved float far outside any window.
            if let Some(preset) = rig.presets.user.first_mut() {
                for slot in &mut preset.layout.dock {
                    if let SavedPlacement::Floating { x, y } = &mut slot.placement {
                        *x = 1.0e6;
                        *y = 1.0e6;
                    }
                }
            }
            rig.run(WorkspaceCommand::Switch(ESSENTIALS.to_owned()));
            let _ = rig.focus.focus(&mut rig.ws.tree, rig.ws.layers.root);
            rig.run(WorkspaceCommand::Switch("far".to_owned()));
            lay(&mut rig.ws, scale, logical);
            assert_eq!(rig.presets.active_name(), "Far");
            assert_eq!(
                rig.focus.focused(),
                Some(rig.ws.layers.root),
                "scale {scale}"
            );
            let hidden = rig
                .ws
                .tree
                .accessibility(rig.ws.layers.root)
                .map(accesskit::Node::is_hidden);
            assert_eq!(hidden, Some(false));
            let index = rig.ws.floating_index_of(rig.ws.layers);
            let frame = index.and_then(|i| rig.ws.floating.get(i)).map(|f| f.frame);
            let (Some(frame), Some(canvas)) = (
                frame.and_then(|f| rig.ws.tree.bounds(f)),
                rig.ws.tree.bounds(rig.ws.canvas_area),
            ) else {
                unreachable!("scale {scale}: Layers floats and is laid out");
            };
            assert!(
                frame.x >= canvas.x
                    && frame.y >= canvas.y
                    && frame.x + i64::from(frame.width) <= canvas.x + i64::from(canvas.width)
                    && frame.y + i64::from(frame.height) <= canvas.y + i64::from(canvas.height),
                "scale {scale}: {frame:?} inside {canvas:?}"
            );

            // A preset that collapses the rail moves focus off it.
            rig.run(WorkspaceCommand::Switch(ESSENTIALS.to_owned()));
            if let Err(err) = aurora_ui::set_rail_collapsed(&mut rig.ws, true) {
                unreachable!("{err:?}");
            }
            rig.save("Collapsed");
            rig.run(WorkspaceCommand::Switch(ESSENTIALS.to_owned()));
            let _ = rig.focus.focus(&mut rig.ws.tree, rig.ws.properties.root);
            rig.run(WorkspaceCommand::Switch("Collapsed".to_owned()));
            lay(&mut rig.ws, scale, logical);
            assert!(aurora_ui::rail_collapsed(&rig.ws));
            let focused = rig.focus.focused();
            assert!(
                focused.is_some_and(|f| !rig.ws.tree.is_within(rig.ws.rail, f)),
                "scale {scale}: focus left the hidden rail ({focused:?})"
            );
        }
    }

    /// AC-4: Reset restores the active preset's *saved* state; edits after
    /// a switch never touch the saved preset.
    #[test]
    fn reset_restores_the_saved_preset_and_edits_never_touch_it() {
        let mut rig = Rig::new();
        set_width(&mut rig.ws, 390.0);
        lay(&mut rig.ws, 1.0, WINDOW);
        rig.save("Paint");
        let saved = rig.presets.layout_of("Paint");
        rig.run(WorkspaceCommand::Switch(ESSENTIALS.to_owned()));
        rig.run(WorkspaceCommand::Switch("Paint".to_owned()));
        edit_everything(&mut rig);
        assert_eq!(rig.presets.layout_of("Paint"), saved, "edits are live only");
        assert!(rig.palette_titles().iter().any(|t| t == "Reset Paint"));
        let outcome = rig.run(WorkspaceCommand::ResetActive);
        assert!(outcome.applied && !outcome.presets_changed);
        assert_eq!(Some(live_layout(&rig.ws)), saved);
        assert_eq!(rig.presets.active_name(), "Paint");
        // Reset Essentials is Essentials.
        rig.run(WorkspaceCommand::Switch(ESSENTIALS.to_owned()));
        edit_everything(&mut rig);
        rig.run(WorkspaceCommand::ResetActive);
        assert_eq!(live_layout(&rig.ws), essentials_layout());
    }

    /// AC-5: Essentials cannot be deleted; deleting the active preset makes
    /// Essentials active and leaves the live layout alone.
    #[test]
    fn delete_refuses_essentials_and_falls_back_from_the_active_preset() {
        let mut rig = Rig::new();
        assert!(
            !rig.run(WorkspaceCommand::Delete("Essentials".to_owned()))
                .presets_changed
        );
        assert!(
            !rig.run(WorkspaceCommand::Delete(" essentials".to_owned()))
                .presets_changed
        );
        assert!(
            !rig.run(WorkspaceCommand::Delete("Nope".to_owned()))
                .presets_changed
        );
        rig.save("Paint");
        edit_everything(&mut rig);
        rig.save("Ink");
        let live = live_layout(&rig.ws);
        assert!(
            rig.run(WorkspaceCommand::Delete("paint".to_owned()))
                .presets_changed
        );
        assert_eq!(rig.presets.active_name(), "Ink", "not the active one");
        let outcome = rig.run(WorkspaceCommand::Delete("Ink".to_owned()));
        assert_eq!(
            outcome,
            WorkspaceCommandOutcome {
                applied: false,
                presets_changed: true
            }
        );
        assert_eq!(rig.presets.active_name(), ESSENTIALS);
        assert!(rig.presets.user().is_empty());
        assert_eq!(live_layout(&rig.ws), live, "the live layout is unchanged");
        assert!(rig.presets.layout_of(ESSENTIALS).is_some());
    }

    fn sample_presets() -> WorkspacePresets {
        let mut rig = Rig::new();
        edit_everything(&mut rig);
        rig.save("Float");
        rig.run(WorkspaceCommand::Switch(ESSENTIALS.to_owned()));
        set_width(&mut rig.ws, 320.0);
        lay(&mut rig.ws, 1.0, WINDOW);
        rig.save("Wide");
        rig.presets
    }

    /// AC-6: presets and the active name round-trip through the file
    /// (written by temporary file and rename); a missing file is Essentials.
    #[test]
    fn presets_round_trip_through_the_file() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let layout_path = dir.path().join("workspace-layout.postcard");
        let path = presets_path_for(&layout_path);
        assert_eq!(path.parent(), layout_path.parent());
        assert_eq!(
            load_presets(&path),
            WorkspacePresets::default(),
            "no file yet"
        );
        let presets = sample_presets();
        assert_eq!(presets.user().len(), 2);
        if let Err(err) = save_presets(&path, &presets) {
            unreachable!("{err}");
        }
        assert_eq!(load_presets(&path), presets);
        assert!(!super::temporary_path(&path).exists(), "renamed into place");
        // Saved again over the old one.
        let mut fewer = presets.clone();
        assert!(fewer.delete("Wide"));
        if let Err(err) = save_presets(&path, &fewer) {
            unreachable!("{err}");
        }
        let loaded = load_presets(&path);
        assert_eq!(loaded, fewer);
        assert_eq!(loaded.active_name(), ESSENTIALS);
    }

    fn encode<T: serde::Serialize>(value: &T) -> Vec<u8> {
        match postcard::to_allocvec(value) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err}"),
        }
    }

    fn file(active: &str, entries: &[(String, Vec<u8>)]) -> Vec<u8> {
        let mut bytes = encode(&PresetsHeader {
            magic: PRESETS_MAGIC,
            version: super::PRESETS_VERSION,
            active: active.to_owned(),
            count: u32::try_from(entries.len()).unwrap_or(0),
        });
        for (name, layout) in entries {
            bytes.extend(encode(&SavedPreset {
                name: name.clone(),
                layout: layout.clone(),
            }));
        }
        bytes
    }

    /// AC-6: damaged data never fails — truncated at every length, garbage,
    /// another magic or version; bad entries are dropped alone, later ones
    /// kept; arrangements repaired; older layout shapes inside a preset
    /// decode through the live layout's fallback chain; a dangling active
    /// name falls back to Essentials.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn damaged_presets_repair_or_fall_back_and_never_fail() {
        let presets = sample_presets();
        let good = match encode_presets(&presets) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err}"),
        };
        assert_eq!(decode_presets(&good), (presets.clone(), false));
        let mut kept_counts = Vec::new();
        for length in 0..good.len() {
            let prefix = good.get(..length).unwrap_or_default();
            let (decoded, repaired) = decode_presets(prefix);
            assert!(repaired || length == 0, "length {length}");
            assert!(decoded.user().len() < 2, "length {length}");
            for preset in decoded.user() {
                assert_eq!(
                    presets.layout_of(&preset.name).as_ref(),
                    Some(&preset.layout)
                );
            }
            kept_counts.push(decoded.user().len());
        }
        assert!(
            kept_counts.contains(&1),
            "a truncated second entry keeps the first"
        );
        for garbage in [
            vec![0xff; 64],
            vec![0x00; 3],
            b"not a presets file".to_vec(),
        ] {
            let (decoded, repaired) = decode_presets(&garbage);
            assert!(repaired);
            assert_eq!(decoded, WorkspacePresets::default());
        }
        let mut wrong_version = encode(&PresetsHeader {
            magic: PRESETS_MAGIC,
            version: 2,
            active: String::new(),
            count: 0,
        });
        wrong_version.extend(good.iter().skip(8));
        assert_eq!(
            decode_presets(&wrong_version),
            (WorkspacePresets::default(), true)
        );
        let wrong_magic = encode(&PresetsHeader {
            magic: *b"XXXX",
            version: super::PRESETS_VERSION,
            active: String::new(),
            count: 0,
        });
        assert_eq!(
            decode_presets(&wrong_magic),
            (WorkspacePresets::default(), true)
        );

        // Bad entries dropped alone; older layout shapes still decode.
        let essentials = encode(&essentials_layout());
        let v1 = encode(&WorkspaceLayoutV1 {
            rail_width: 300.0,
            layers_collapsed: true,
            properties_collapsed: false,
            history_collapsed: false,
        });
        let v4 = encode(&WorkspaceLayoutV4 {
            rail_width: 280.0,
            layers_collapsed: false,
            properties_collapsed: false,
            history_collapsed: false,
            panel_group_tab: 1,
            rail_collapsed: true,
            dock: crate::default_saved_dock(1),
        });
        let mut damaged_dock = essentials_layout();
        damaged_dock.rail_width = f32::NAN;
        if let Some(slot) = damaged_dock.dock.get_mut(0) {
            slot.panels = vec![
                "history".to_owned(),
                "history".to_owned(),
                "nope".to_owned(),
            ];
        }
        damaged_dock.float_stack = vec![7, 7];
        let bytes = file(
            "Gone",
            &[
                (String::new(), essentials.clone()),
                ("Essentials".to_owned(), essentials.clone()),
                ("Old".to_owned(), v1),
                ("old".to_owned(), essentials.clone()),
                ("Broken".to_owned(), vec![0xff, 0xff, 0xff]),
                ("x".repeat(WORKSPACE_NAME_MAX_CHARS + 1), essentials.clone()),
                ("Mid".to_owned(), v4),
                (" Repaired ".to_owned(), encode(&damaged_dock)),
            ],
        );
        let (decoded, repaired) = decode_presets(&bytes);
        assert!(repaired);
        let names: Vec<&str> = decoded.user().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Old", "Mid", "Repaired"]);
        assert_eq!(decoded.active_name(), ESSENTIALS, "a dangling active name");
        let old = decoded.layout_of("Old");
        assert_eq!(old.as_ref().map(|l| l.rail_width), Some(300.0));
        assert_eq!(old.as_ref().map(|l| l.layers_collapsed), Some(true));
        let mid = decoded.layout_of("Mid");
        assert_eq!(mid.as_ref().map(|l| l.rail_collapsed), Some(true));
        assert!(mid.as_ref().is_some_and(|l| l.float_stack.is_empty()));
        let Some(fixed) = decoded.layout_of("Repaired") else {
            unreachable!("kept");
        };
        assert!(fixed.rail_width.is_finite());
        let mut keys: Vec<String> = fixed.dock.iter().flat_map(|s| s.panels.clone()).collect();
        keys.sort();
        let mut every: Vec<String> = DockPanel::ALL.iter().map(|p| p.key().to_owned()).collect();
        every.sort();
        assert_eq!(keys, every, "every panel exactly once");
        // A repaired preset still applies cleanly.
        let mut rig = Rig::new();
        rig.presets = decoded;
        assert!(
            rig.run(WorkspaceCommand::Switch("Repaired".to_owned()))
                .applied
        );
        assert_eq!(
            rig.ws.dock_arrangement().slots().len() + rig.ws.floating.len(),
            {
                let arrangement = rig.ws.dock_arrangement();
                arrangement.slots().len() + arrangement.floating().len()
            }
        );
        // The active name, matched case-insensitively, survives.
        let bytes = file("mid", &[("Mid".to_owned(), essentials)]);
        assert_eq!(decode_presets(&bytes).0.active_name(), "Mid");
    }

    /// AC-7: the name prompt is a focused, named `TextInput`; Escape
    /// cancels (nothing saved, focus repaired); Enter confirms.
    #[test]
    fn the_name_prompt_is_keyboard_and_at_accessible() {
        let mut rig = Rig::new();
        let _ = rig.focus.focus(&mut rig.ws.tree, rig.ws.layers.root);
        rig.run(WorkspaceCommand::PromptSaveAs);
        let Some(root) = rig.palette else {
            unreachable!("opened");
        };
        assert_eq!(rig.focus.focused(), Some(root), "focus on open");
        let (label, description, rows) = rig.prompt();
        assert_eq!(label, WORKSPACE_NAME_PROMPT_LABEL);
        assert_eq!(description, None);
        assert_eq!(
            rows,
            vec![(
                COMMAND_WORKSPACE_NAME_SAVE.to_owned(),
                "Save Workspace".to_owned()
            )]
        );
        assert!(
            rig.ws
                .tree
                .bounds(root)
                .is_some_and(|b| b.width > 0 && b.height > 0)
        );
        rig.type_text("My Look");
        let node = rig.ws.tree.accessibility(root).cloned();
        assert_eq!(node.as_ref().and_then(|n| n.value()), Some("My Look"));
        let (_, _, rows) = rig.prompt();
        assert_eq!(
            rows.first().map(|r| r.1.as_str()),
            Some("Save Workspace \u{201c}My Look\u{201d}")
        );
        // A second prompt or palette cannot stack on it.
        assert!(!super::open_workspace_name_prompt(
            &mut rig.ws,
            &mut rig.focus,
            &mut rig.palette,
            "",
            false
        ));
        assert_eq!(rig.key(Key::Named(NamedKey::Escape), None), None);
        assert!(rig.palette.is_none());
        assert!(rig.ws.tree.accessibility(root).is_none(), "removed");
        assert!(rig.focus.focused().is_none_or(|f| f != root));
        assert!(rig.presets.user().is_empty(), "Escape saved nothing");

        rig.run(WorkspaceCommand::PromptSaveAs);
        rig.type_text("My Lookx");
        assert_eq!(rig.key(Key::Named(NamedKey::Backspace), None), None);
        let picked = rig.key(Key::Named(NamedKey::Enter), None);
        assert_eq!(
            picked,
            Some(ActivatedCommand::Workspace(WorkspaceCommand::Save {
                name: "My Look".to_owned(),
                replace: false
            }))
        );
        assert!(rig.palette.is_none(), "Enter closes it");
        let Some(ActivatedCommand::Workspace(command)) = picked else {
            unreachable!();
        };
        rig.run(command);
        assert_eq!(rig.presets.active_name(), "My Look");
    }
}
