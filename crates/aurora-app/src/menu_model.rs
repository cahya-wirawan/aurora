//! The in-window menu bar's model (0.225.0): one pure data table of menus
//! and items, with no widget, window or `App` in it.
//!
//! Every item names an **existing command id** — the same `COMMAND_*`
//! constants the command palette lists and `activate_command` resolves,
//! and that the macOS native menu is built from — so the bar, the palette
//! and the native menu drive one command vocabulary and nothing here
//! invents a second one. Photoshop's submenus are **flattened**: the
//! Select menu's Modify entries read "Modify: Feather…", the selection
//! modes "Selection Mode: Add", and so on. Three sections are dynamic
//! (workspaces, open documents, saved selections); they arrive in
//! [`MenuContext`] as the palette's own [`CommandEntry`] lists, so a
//! workspace added or a document opened shows up with no second source.
//!
//! **Shortcut hints** are read from the live shortcut registry
//! ([`hint_command`] says which `AppCommand` stands behind an id), never
//! typed here. **Availability** reuses the rules the app already
//! applies ([`MenuContext`]'s fields are those facts): Undo/Redo need a
//! step, Delete Layer needs `delete_allowed`, a clipping mask needs
//! `clip_refusal` to be `None`, and the Select commands, Modify and Load
//! Selection are refused while Quick Mask is on. A disabled item is shown
//! disabled and does not run.

use aurora_widgets::ShortcutRegistry;
use aurora_widgets::widgets::{CommandEntry, MenuItem};

use crate::AppCommand;

/// What a menu entry is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    Separator,
    Item {
        /// The command id `activate_command` resolves.
        command: String,
        label: String,
        /// The shortcut's text, from the registry, if the command has one.
        hint: Option<String>,
        enabled: bool,
    },
}

/// One menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Menu {
    pub(crate) title: &'static str,
    pub(crate) entries: Vec<Entry>,
}

/// The app facts the menus depend on, gathered by `App` once per open.
// Independent facts, not one state: a flag each.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default)]
pub(crate) struct MenuContext {
    pub(crate) can_undo: bool,
    pub(crate) can_redo: bool,
    /// `delete_allowed` for the active layer.
    pub(crate) delete_allowed: bool,
    /// Quick Mask mode is on.
    pub(crate) quick_mask: bool,
    /// The active layer is clipped.
    pub(crate) clipped: bool,
    /// `clip_refusal` for the active layer is `Some` (a group, or the
    /// bottom layer: nothing to clip to), or there is no active layer.
    pub(crate) clip_refused: bool,
    /// `workspace_presets::workspace_palette_entries`.
    pub(crate) workspaces: Vec<CommandEntry>,
    /// `document_tabs::document_palette_entries`.
    pub(crate) documents: Vec<CommandEntry>,
    /// `selection_store::palette_entries`.
    pub(crate) saved_selections: Vec<CommandEntry>,
}

/// A static row: a separator or a command with its label.
enum Row {
    Sep,
    /// A registry command; its label is the registry title.
    Cmd(&'static str),
}

use Row::{Cmd, Sep};

/// The ids of the Select menu's Modify rows, from `RefineKind`.
fn modify_rows() -> Vec<(&'static str, String)> {
    crate::selection_refine::RefineKind::ALL
        .iter()
        .map(|kind| {
            let name = kind.menu_label().trim_end_matches('\u{2026}').trim_end();
            let name = name.trim_end_matches(" Selection");
            (kind.command_id(), format!("Modify: {name}\u{2026}"))
        })
        .collect()
}

#[allow(clippy::too_many_lines)] // one row per menu command
fn static_rows(title: &str) -> Vec<Row> {
    match title {
        "File" => vec![
            Cmd(crate::document_tabs::COMMAND_FILE_NEW),
            Cmd(crate::COMMAND_FILE_OPEN),
            Cmd(crate::COMMAND_FILE_SAVE),
            Cmd(crate::psd_save::COMMAND_CHECK),
            Sep,
            Cmd(crate::document_close::COMMAND_FILE_CLOSE),
            Sep,
            Cmd(crate::COMMAND_APP_QUIT),
        ],
        "Edit" => vec![
            Cmd(crate::COMMAND_UNDO),
            Cmd(crate::COMMAND_REDO),
            Sep,
            Cmd(crate::clipboard_app::COMMAND_COPY),
            Cmd(crate::clipboard_app::COMMAND_CUT),
            Cmd(crate::clipboard_app::COMMAND_COPY_MERGED),
            Cmd(crate::clipboard_app::COMMAND_PASTE),
            Cmd(crate::clipboard_app::COMMAND_PASTE_IN_PLACE),
            Cmd(crate::clipboard_app::COMMAND_LAYER_VIA_COPY),
            Cmd(crate::clipboard_app::COMMAND_LAYER_VIA_CUT),
            Sep,
            Cmd(crate::COMMAND_COLOURS_SWAP),
            Cmd(crate::COMMAND_COLOURS_RESET),
            Sep,
            Cmd(crate::colour_app::COMMAND_SET_OUTPUT),
            Cmd(crate::colour_app::COMMAND_CLEAR_OUTPUT),
        ],
        "Layer" => vec![
            Cmd(crate::COMMAND_LAYER_NEW),
            Cmd(crate::COMMAND_LAYER_NEW_GROUP),
            Cmd(crate::COMMAND_LAYER_DELETE),
            Sep,
            Cmd(crate::COMMAND_LAYER_NEW_CURVES),
            Cmd(crate::COMMAND_LAYER_NEW_LEVELS),
            Cmd(crate::COMMAND_LAYER_NEW_HUE_SATURATION),
            Cmd(crate::COMMAND_LAYER_NEW_VIBRANCE),
            Cmd(crate::COMMAND_LAYER_NEW_EXPOSURE),
            Cmd(crate::COMMAND_LAYER_NEW_PHOTO_FILTER),
            Cmd(crate::COMMAND_LAYER_NEW_BLACK_WHITE),
            Cmd(crate::COMMAND_LAYER_NEW_COLOR_BALANCE),
            Cmd(crate::COMMAND_LAYER_NEW_INVERT),
            Cmd(crate::COMMAND_LAYER_NEW_THRESHOLD),
            Cmd(crate::COMMAND_LAYER_NEW_POSTERIZE),
            Cmd(crate::COMMAND_LAYER_NEW_BRIGHTNESS_CONTRAST),
            Cmd(crate::COMMAND_LAYER_NEW_CHANNEL_MIXER),
            Cmd(crate::COMMAND_LAYER_NEW_SELECTIVE_COLOR),
            Cmd(crate::COMMAND_LAYER_NEW_GRADIENT_MAP),
            Sep,
            Cmd(crate::COMMAND_LAYER_CLIP_CREATE),
            Cmd(crate::COMMAND_LAYER_CLIP_RELEASE),
        ],
        "View" => vec![
            Cmd(crate::zoomed_view::COMMAND_FIT_ON_SCREEN),
            Cmd(crate::zoomed_view::COMMAND_ACTUAL_PIXELS),
            Sep,
            Cmd(crate::COMMAND_FOCUS_LAYERS),
            Cmd(crate::COMMAND_FOCUS_PROPERTIES),
            Cmd(crate::COMMAND_FOCUS_HISTORY),
            Cmd(crate::COMMAND_FOCUS_CHANNELS),
            Cmd(crate::COMMAND_FOCUS_PATHS),
            Sep,
            Cmd(crate::COMMAND_TOGGLE_LAYERS),
            Cmd(crate::COMMAND_TOGGLE_PROPERTIES),
            Cmd(crate::COMMAND_TOGGLE_HISTORY),
            Cmd(crate::COMMAND_TOGGLE_CHANNELS),
            Cmd(crate::COMMAND_TOGGLE_PATHS),
            Sep,
            Cmd(crate::COMMAND_CLOSE_LAYERS),
            Cmd(crate::COMMAND_CLOSE_PROPERTIES),
            Cmd(crate::COMMAND_CLOSE_HISTORY),
            Cmd(crate::COMMAND_CLOSE_CHANNELS),
            Cmd(crate::COMMAND_CLOSE_PATHS),
            Sep,
            Cmd(crate::COMMAND_TOGGLE_PANELS),
            Cmd(crate::COMMAND_RESET_PANELS),
            Sep,
            Cmd(crate::COMMAND_TOGGLE_WIDGET_GALLERY),
        ],
        _ => Vec::new(),
    }
}

/// The static rows as entries (no hints, everything enabled yet).
fn rows_to_entries(rows: Vec<Row>) -> Vec<Entry> {
    rows.into_iter()
        .map(|row| match row {
            Sep => Entry::Separator,
            Cmd(command) => item(command, registry_title(command)),
        })
        .collect()
}

/// The registry's title for `command` (the id itself for one the registry
/// does not know, which a test forbids).
fn registry_title(command: &str) -> &str {
    let title = crate::command_registry::menu_label(command);
    debug_assert!(
        title.is_some(),
        "menu command {command} is not in the registry"
    );
    title.unwrap_or(command)
}

fn item(command: &str, label: &str) -> Entry {
    Entry::Item {
        command: command.to_owned(),
        label: label.to_owned(),
        hint: None,
        enabled: true,
    }
}

/// Appends `dynamic` entries (a separator first) when there are any.
fn push_dynamic(entries: &mut Vec<Entry>, dynamic: &[CommandEntry]) {
    if dynamic.is_empty() {
        return;
    }
    entries.push(Entry::Separator);
    entries.extend(dynamic.iter().map(|e| item(&e.id, &e.title)));
}

/// A registry command as a menu item, labelled with its registry title.
fn registry_item(command: &str) -> Entry {
    item(command, registry_title(command))
}

fn select_entries(context: &MenuContext) -> Vec<Entry> {
    let mut entries = vec![
        registry_item(crate::COMMAND_SELECT_ALL),
        registry_item(crate::COMMAND_SELECT_DESELECT),
        registry_item(crate::COMMAND_SELECT_INVERSE),
        Entry::Separator,
    ];
    entries.extend(
        modify_rows()
            .into_iter()
            .map(|(id, label)| item(id, &label)),
    );
    entries.push(Entry::Separator);
    entries.push(registry_item(crate::colour_range_app::COMMAND_ID));
    entries.push(registry_item(crate::quick_mask::COMMAND_ID));
    entries.push(registry_item(crate::crop::COMMAND_CROP_TO_SELECTION));
    entries.push(Entry::Separator);
    entries.extend(
        crate::paths_app::COMMANDS
            .iter()
            .map(|(id, _)| registry_item(id)),
    );
    entries.push(Entry::Separator);
    entries.push(registry_item(crate::selection_store::COMMAND_SAVE));
    // Load and Delete per saved name, as the palette lists them.
    for entry in &context.saved_selections {
        entries.push(item(&entry.id, &entry.title));
    }
    entries.push(Entry::Separator);
    entries.extend(
        aurora_doc::SelectionMode::ALL
            .iter()
            .map(|mode| registry_item(crate::selection_mode_command_id(*mode))),
    );
    entries
}

fn window_entries(context: &MenuContext) -> Vec<Entry> {
    let mut entries: Vec<Entry> = context
        .workspaces
        .iter()
        .map(|e| item(&e.id, &e.title))
        .collect();
    entries.push(Entry::Separator);
    entries.push(registry_item(crate::document_tabs::COMMAND_DOCUMENT_NEXT));
    entries.push(registry_item(
        crate::document_tabs::COMMAND_DOCUMENT_PREVIOUS,
    ));
    push_dynamic(&mut entries, &context.documents);
    entries.push(Entry::Separator);
    entries.extend(
        crate::PANEL_FLOAT_COMMANDS
            .iter()
            .map(|(id, _, _, _)| registry_item(id)),
    );
    entries
}

/// Which `AppCommand` a shortcut hint for command `id` is read from.
pub(crate) fn hint_command(id: &str) -> Option<AppCommand> {
    Some(match id {
        crate::COMMAND_UNDO => AppCommand::Undo,
        crate::COMMAND_REDO => AppCommand::Redo,
        crate::document_close::COMMAND_FILE_CLOSE => AppCommand::CloseDocument,
        crate::COMMAND_LAYER_NEW => AppCommand::NewLayer,
        crate::COMMAND_LAYER_CLIP_CREATE | crate::COMMAND_LAYER_CLIP_RELEASE => {
            AppCommand::ToggleClippingMask
        }
        crate::COMMAND_COLOURS_SWAP => AppCommand::SwapColours,
        crate::zoomed_view::COMMAND_FIT_ON_SCREEN => AppCommand::FitOnScreen,
        crate::zoomed_view::COMMAND_ACTUAL_PIXELS => AppCommand::ActualPixels,
        crate::COMMAND_COLOURS_RESET => AppCommand::ResetColours,
        crate::COMMAND_SELECT_ALL => AppCommand::SelectAll,
        _ if crate::clipboard_app::command_for(id).is_some() => {
            return crate::clipboard_app::command_for(id).map(AppCommand::Clipboard);
        }
        crate::COMMAND_SELECT_DESELECT => AppCommand::Deselect,
        crate::COMMAND_SELECT_INVERSE => AppCommand::InverseSelection,
        crate::quick_mask::COMMAND_ID => AppCommand::ToggleQuickMask,
        crate::document_tabs::COMMAND_DOCUMENT_NEXT => AppCommand::NextDocument,
        crate::document_tabs::COMMAND_DOCUMENT_PREVIOUS => AppCommand::PreviousDocument,
        _ => return None,
    })
}

/// Whether command `id` can run now (an item that cannot is shown
/// disabled): the registry's requirements for the id, evaluated on the
/// menu's facts ([`crate::command_registry::enabled`]); an id it does not
/// know is enabled.
pub(crate) fn enabled(id: &str, context: &MenuContext) -> bool {
    crate::command_registry::enabled(id, context)
}

/// Builds the menus, in bar order, with hints from `shortcuts` and
/// availability from `context`.
pub(crate) fn build_menus(
    context: &MenuContext,
    shortcuts: &ShortcutRegistry<AppCommand>,
) -> Vec<Menu> {
    aurora_ui::MENU_BAR_TITLES
        .iter()
        .map(|&title| {
            let mut entries = match title {
                "Select" => select_entries(context),
                "Window" => window_entries(context),
                other => rows_to_entries(static_rows(other)),
            };
            for entry in &mut entries {
                if let Entry::Item {
                    command,
                    hint,
                    enabled: on,
                    ..
                } = entry
                {
                    *on = enabled(command, context);
                    *hint = hint_command(command)
                        .and_then(|app| shortcuts.chords_for(&app).first().copied())
                        .map(|chord| chord.to_string());
                }
            }
            Menu { title, entries }
        })
        .collect()
}

/// The visible text of an item: its label, then its shortcut hint two
/// em-spaces on (the toolkit's menu row draws one line of text; there is no
/// right-aligned hint column yet).
pub(crate) fn display_label(label: &str, hint: Option<&str>) -> String {
    match hint {
        Some(hint) => format!("{label}\u{2003}\u{2003}{hint}"),
        None => label.to_owned(),
    }
}

/// A menu as the toolkit's items plus, parallel to them, the command each
/// action runs (`None` for a separator).
pub(crate) fn widget_items(menu: &Menu) -> (Vec<MenuItem>, Vec<Option<String>>) {
    let mut items = Vec::with_capacity(menu.entries.len());
    let mut commands = Vec::with_capacity(menu.entries.len());
    for entry in &menu.entries {
        match entry {
            Entry::Separator => {
                items.push(MenuItem::separator());
                commands.push(None);
            }
            Entry::Item {
                command,
                label,
                hint,
                enabled,
            } => {
                items.push(MenuItem {
                    enabled: *enabled,
                    ..MenuItem::action(&display_label(label, hint.as_deref()))
                });
                commands.push(Some(command.clone()));
            }
        }
    }
    (items, commands)
}

/// Whether the bar shows: on every platform but macOS (which has its native
/// menu), unless `AURORA_MENU_BAR` says otherwise — `1` (or `true`/`on`/
/// `yes`) forces it on, `0` (or `false`/`off`/`no`) forces it off, anything
/// else (empty included) is "detect".
pub(crate) fn menu_bar_enabled(is_macos: bool, env: Option<&str>) -> bool {
    match env.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if matches!(value.as_str(), "1" | "true" | "on" | "yes") => true,
        Some(value) if matches!(value.as_str(), "0" | "false" | "off" | "no") => false,
        _ => !is_macos,
    }
}

/// The ids the macOS native menu is built from (`build_menu`), kept here
/// so a pure test can hold them against the model on every platform. It is
/// a hand-kept list: nothing walks the real built menu against it
/// (`muda::Menu::new()` needs the main thread), so a change to
/// `build_menu` must update it by hand.
#[cfg(test)]
pub(crate) const NATIVE_MENU_COMMANDS: &[&str] = &[
    crate::zoomed_view::COMMAND_FIT_ON_SCREEN,
    crate::zoomed_view::COMMAND_ACTUAL_PIXELS,
    crate::COMMAND_FILE_OPEN,
    crate::COMMAND_FILE_SAVE,
    crate::psd_save::COMMAND_CHECK,
    crate::document_close::COMMAND_FILE_CLOSE,
    crate::COMMAND_UNDO,
    crate::COMMAND_REDO,
    crate::COMMAND_SELECT_ALL,
    crate::COMMAND_SELECT_DESELECT,
    crate::COMMAND_SELECT_INVERSE,
    crate::quick_mask::COMMAND_ID,
    crate::crop::COMMAND_CROP_TO_SELECTION,
    crate::colour_range_app::COMMAND_ID,
    crate::clipboard_app::COMMAND_COPY,
    crate::clipboard_app::COMMAND_CUT,
    crate::clipboard_app::COMMAND_COPY_MERGED,
    crate::clipboard_app::COMMAND_PASTE,
    crate::clipboard_app::COMMAND_PASTE_IN_PLACE,
    crate::clipboard_app::COMMAND_LAYER_VIA_COPY,
    crate::clipboard_app::COMMAND_LAYER_VIA_CUT,
    crate::paths_app::COMMAND_MAKE_SELECTION,
    crate::path_paint::COMMAND_FILL,
    crate::path_paint::COMMAND_STROKE,
    crate::path_paint::COMMAND_MAKE_FROM_SELECTION,
    crate::selection_refine::RefineKind::Feather.command_id(),
    crate::selection_refine::RefineKind::Expand.command_id(),
    crate::selection_refine::RefineKind::Contract.command_id(),
    crate::selection_refine::RefineKind::Smooth.command_id(),
    crate::selection_refine::RefineKind::Border.command_id(),
    crate::COMMAND_LAYER_NEW,
    crate::COMMAND_LAYER_DELETE,
    crate::COMMAND_LAYER_NEW_CURVES,
    crate::COMMAND_LAYER_NEW_LEVELS,
    crate::COMMAND_LAYER_NEW_HUE_SATURATION,
    crate::COMMAND_LAYER_NEW_VIBRANCE,
    crate::COMMAND_LAYER_NEW_EXPOSURE,
    crate::COMMAND_LAYER_NEW_PHOTO_FILTER,
    crate::COMMAND_LAYER_NEW_BLACK_WHITE,
    crate::COMMAND_LAYER_NEW_COLOR_BALANCE,
    crate::COMMAND_LAYER_NEW_INVERT,
    crate::COMMAND_LAYER_NEW_THRESHOLD,
    crate::COMMAND_LAYER_NEW_POSTERIZE,
    crate::COMMAND_LAYER_NEW_BRIGHTNESS_CONTRAST,
    crate::COMMAND_LAYER_NEW_CHANNEL_MIXER,
    crate::COMMAND_LAYER_NEW_SELECTIVE_COLOR,
    crate::COMMAND_LAYER_NEW_GRADIENT_MAP,
    crate::COMMAND_LAYER_NEW_GROUP,
    crate::COMMAND_LAYER_CLIP_CREATE,
    crate::COMMAND_LAYER_CLIP_RELEASE,
    crate::COMMAND_FOCUS_LAYERS,
    crate::COMMAND_FOCUS_PROPERTIES,
    crate::COMMAND_FOCUS_HISTORY,
    crate::COMMAND_FOCUS_CHANNELS,
    crate::COMMAND_FOCUS_PATHS,
    crate::COMMAND_TOGGLE_LAYERS,
    crate::COMMAND_TOGGLE_PROPERTIES,
    crate::COMMAND_TOGGLE_HISTORY,
    crate::COMMAND_TOGGLE_CHANNELS,
    crate::COMMAND_TOGGLE_PATHS,
    crate::COMMAND_CLOSE_LAYERS,
    crate::COMMAND_CLOSE_PROPERTIES,
    crate::COMMAND_CLOSE_HISTORY,
    crate::COMMAND_CLOSE_CHANNELS,
    crate::COMMAND_CLOSE_PATHS,
    crate::COMMAND_TOGGLE_WIDGET_GALLERY,
    crate::COMMAND_TOGGLE_PANELS,
    crate::COMMAND_APP_QUIT,
];
