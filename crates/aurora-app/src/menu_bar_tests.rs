//! Tests for the in-window menu bar (0.225.0): the pure model
//! ([`crate::menu_model`]), the state machine ([`crate::menu_bar_state`])
//! driven through the production routers (`route_widget_pointer`,
//! `route_widget_key`) on a headless workspace, and the command table
//! cross-checks. `App` needs a window, so its glue (`menu_bar_app`) is read,
//! not run; everything it calls is exercised here.

#![allow(clippy::indexing_slicing)] // fixtures built a few lines up

use aurora_widgets::shortcut::{Key, Modifiers, NamedKey};
use aurora_widgets::widgets::{CommandEntry, WidgetKind};
use aurora_widgets::{ActionOutcome, FocusManager, PointerOutcome, PointerPhase, WidgetId};

use crate::menu_bar_state::{BarAction, KeyAction, MenuBarState, wrap_step};
use crate::menu_model::{self, Entry, Menu, MenuContext};
use crate::{AppCommand, default_shortcuts_for};

fn context() -> MenuContext {
    MenuContext {
        can_undo: true,
        can_redo: true,
        delete_allowed: true,
        quick_mask: false,
        clipped: false,
        clip_refused: false,
        workspaces: crate::workspace_presets::workspace_palette_entries(
            &crate::workspace_presets::WorkspacePresets::default(),
        ),
        documents: vec![
            CommandEntry::new("document.switch.7", "Document: a.png"),
            CommandEntry::new("document.switch.9", "Document: b.png"),
        ],
        saved_selections: vec![
            CommandEntry::new("select.load:sky", "Load Selection: sky"),
            CommandEntry::new("select.delete_saved:sky", "Delete Saved Selection: sky"),
        ],
    }
}

fn menus_for(context: &MenuContext) -> Vec<Menu> {
    menu_model::build_menus(context, &default_shortcuts_for(false))
}

fn menu<'a>(menus: &'a [Menu], title: &str) -> &'a Menu {
    match menus.iter().find(|menu| menu.title == title) {
        Some(menu) => menu,
        None => unreachable!("no {title} menu"),
    }
}

fn entry<'a>(menu: &'a Menu, command: &str) -> (&'a str, Option<&'a str>, bool) {
    for entry in &menu.entries {
        if let Entry::Item {
            command: id,
            label,
            hint,
            enabled,
        } = entry
            && id == command
        {
            return (label.as_str(), hint.as_deref(), *enabled);
        }
    }
    unreachable!("{} has no {command}", menu.title)
}

fn all_commands(menus: &[Menu]) -> Vec<String> {
    menus
        .iter()
        .flat_map(|menu| menu.entries.iter())
        .filter_map(|entry| match entry {
            Entry::Item { command, .. } => Some(command.clone()),
            Entry::Separator => None,
        })
        .collect()
}

// -- the model --

#[test]
fn the_model_has_the_bars_six_menus_in_order() {
    let menus = menus_for(&context());
    let titles: Vec<&str> = menus.iter().map(|m| m.title).collect();
    assert_eq!(titles, aurora_ui::MENU_BAR_TITLES);
    assert_eq!(
        titles,
        ["File", "Edit", "Layer", "Select", "View", "Window"]
    );
}

struct PathDialog;
impl crate::FileDialogAccess for PathDialog {
    fn pick_file(&mut self) -> Option<std::path::PathBuf> {
        Some("x.png".into())
    }
    fn save_file(&mut self) -> Option<std::path::PathBuf> {
        Some("y.aur".into())
    }
}

/// Whether `activate_command` knows `id`: it returns a command, or it acted
/// on a panel (focus, collapse, close, the rail toggle) and returned none.
fn resolves(id: &str) -> bool {
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    workspace.tree.compute_layout(1280.0, 800.0);
    let panel_command = crate::command_target(&workspace, id).is_some()
        || crate::command_collapse_target(&workspace, id).is_some()
        || crate::command_close_target(&workspace, id).is_some()
        || id == crate::COMMAND_TOGGLE_PANELS;
    let mut focus = FocusManager::default();
    let picked = crate::activate_command(&mut workspace, &mut focus, id, &mut PathDialog);
    picked.is_some() || panel_command
}

#[test]
fn every_menu_item_resolves_in_activate_command_so_none_is_dead() {
    let menus = menus_for(&context());
    let ids = all_commands(&menus);
    assert!(ids.len() > 50, "{} items", ids.len());
    for id in &ids {
        assert!(resolves(id), "{id} is a dead menu item");
    }
    // The check itself can fail: an unknown id does not resolve.
    assert!(!resolves("menu.nonsense"));
}

#[test]
fn every_static_item_is_also_in_the_command_palette() {
    let palette: Vec<String> = crate::palette_commands()
        .into_iter()
        .map(|e| e.id)
        .collect();
    let dynamic = [
        "workspace.",
        "document.switch.",
        "select.load:",
        "select.delete_saved",
    ];
    for id in all_commands(&menus_for(&context())) {
        if dynamic.iter().any(|p| id.starts_with(p)) || id == crate::COMMAND_APP_QUIT {
            continue;
        }
        assert!(
            palette.contains(&id),
            "{id} is in the menu but not the palette"
        );
    }
}

#[test]
fn the_native_menu_ids_are_a_subset_of_the_models() {
    let ids = all_commands(&menus_for(&context()));
    assert!(menu_model::NATIVE_MENU_COMMANDS.len() > 30);
    for native in menu_model::NATIVE_MENU_COMMANDS {
        assert!(
            ids.iter().any(|id| id == native),
            "{native} missing from the model"
        );
    }
}

#[test]
fn no_menu_has_an_empty_label_or_a_stray_separator() {
    for menu in menus_for(&context()) {
        assert!(!menu.entries.is_empty(), "{}", menu.title);
        assert!(
            matches!(menu.entries.first(), Some(Entry::Item { .. })),
            "{}",
            menu.title
        );
        assert!(
            matches!(menu.entries.last(), Some(Entry::Item { .. })),
            "{}",
            menu.title
        );
        let mut previous_separator = false;
        for entry in &menu.entries {
            match entry {
                Entry::Separator => {
                    assert!(
                        !previous_separator,
                        "{}: two separators in a row",
                        menu.title
                    );
                    previous_separator = true;
                }
                Entry::Item { label, command, .. } => {
                    assert!(
                        !label.trim().is_empty() && !command.is_empty(),
                        "{}",
                        menu.title
                    );
                    previous_separator = false;
                }
            }
        }
    }
}

#[test]
fn a_command_appears_once_per_menu_and_modify_rows_are_flattened() {
    let menus = menus_for(&context());
    for menu in &menus {
        let mut ids = all_commands(std::slice::from_ref(menu));
        let before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), before, "{} repeats a command", menu.title);
    }
    let select = menu(&menus, "Select");
    assert_eq!(entry(select, "select.feather").0, "Modify: Feather\u{2026}");
    assert_eq!(entry(select, "select.border").0, "Modify: Border\u{2026}");
    assert_eq!(entry(select, "selection.mode.add").0, "Selection Mode: Add");
}

#[test]
fn the_dynamic_sections_follow_the_context() {
    let menus = menus_for(&context());
    let window = menu(&menus, "Window");
    assert_eq!(
        entry(window, "workspace.switch/Essentials").0,
        "Workspace: Essentials"
    );
    assert_eq!(entry(window, "document.switch.7").0, "Document: a.png");
    assert_eq!(entry(window, "document.switch.9").0, "Document: b.png");
    let select = menu(&menus, "Select");
    assert_eq!(entry(select, "select.load:sky").0, "Load Selection: sky");
    assert_eq!(
        entry(select, "select.delete_saved:sky").0,
        "Delete Saved Selection: sky"
    );
    // Nothing saved, no documents: those rows are simply absent.
    let bare = menus_for(&MenuContext {
        workspaces: context().workspaces,
        ..MenuContext::default()
    });
    let ids = all_commands(&bare);
    assert!(!ids.iter().any(|id| id.starts_with("document.switch.")));
    assert!(!ids.iter().any(|id| id.starts_with("select.load:")));
    assert!(ids.iter().any(|id| id == "document.next"));
    // A new workspace preset appears in the Window menu in order.
    let mut more = context();
    more.workspaces.insert(
        1,
        CommandEntry::new("workspace.switch/Mine", "Workspace: Mine"),
    );
    let more_menus = menus_for(&more);
    assert_eq!(
        entry(menu(&more_menus, "Window"), "workspace.switch/Mine").0,
        "Workspace: Mine"
    );
}

#[test]
fn availability_follows_the_apps_own_rules() {
    let mut ctx = context();
    ctx.can_undo = false;
    ctx.can_redo = false;
    ctx.delete_allowed = false;
    ctx.clip_refused = true;
    let menus = menus_for(&ctx);
    assert!(!entry(menu(&menus, "Edit"), "edit.undo").2);
    assert!(!entry(menu(&menus, "Edit"), "edit.redo").2);
    assert!(!entry(menu(&menus, "Layer"), "layer.delete").2);
    assert!(!entry(menu(&menus, "Layer"), "layer.clip_create").2);
    assert!(
        !entry(menu(&menus, "Layer"), "layer.clip_release").2,
        "nothing clipped"
    );
    assert!(entry(menu(&menus, "Layer"), "layer.new").2);
    let menus = menus_for(&context());
    assert!(entry(menu(&menus, "Edit"), "edit.undo").2);
    assert!(entry(menu(&menus, "Edit"), "edit.redo").2);
    assert!(entry(menu(&menus, "Layer"), "layer.delete").2);
    assert!(entry(menu(&menus, "Layer"), "layer.clip_create").2);
    let mut clipped = context();
    clipped.clipped = true;
    let menus = menus_for(&clipped);
    assert!(
        !entry(menu(&menus, "Layer"), "layer.clip_create").2,
        "already clipped"
    );
    assert!(entry(menu(&menus, "Layer"), "layer.clip_release").2);
}

#[test]
fn quick_mask_disables_exactly_the_commands_it_refuses() {
    let mut ctx = context();
    ctx.quick_mask = true;
    let menus = menus_for(&ctx);
    let select = menu(&menus, "Select");
    for id in [
        "select.all",
        "select.deselect",
        "select.inverse",
        "select.feather",
        "select.expand",
        "select.contract",
        "select.smooth",
        "select.border",
        "select.load:sky",
    ] {
        assert!(!entry(select, id).2, "{id} is refused in Quick Mask");
    }
    for id in [
        "select.quick_mask",
        "select.save",
        "select.delete_saved:sky",
        "selection.mode.add",
    ] {
        assert!(entry(select, id).2, "{id} still works in Quick Mask");
    }
    let normal = menus_for(&context());
    assert!(entry(menu(&normal, "Select"), "select.all").2);
    assert!(entry(menu(&normal, "Select"), "select.load:sky").2);
}

#[test]
fn shortcut_hints_come_from_the_live_registry() {
    let menus = menus_for(&context());
    assert_eq!(entry(menu(&menus, "Edit"), "edit.undo").1, Some("Ctrl+Z"));
    assert_eq!(
        entry(menu(&menus, "Edit"), "edit.redo").1,
        Some("Ctrl+Shift+Z")
    );
    assert_eq!(
        entry(menu(&menus, "Select"), "select.all").1,
        Some("Ctrl+A")
    );
    assert_eq!(
        entry(menu(&menus, "Select"), "select.inverse").1,
        Some("Ctrl+Shift+I")
    );
    assert_eq!(
        entry(menu(&menus, "Select"), "select.quick_mask").1,
        Some("Q")
    );
    assert_eq!(
        entry(menu(&menus, "Layer"), "layer.new").1,
        Some("Ctrl+Shift+N")
    );
    assert_eq!(entry(menu(&menus, "File"), "file.close").1, Some("Ctrl+W"));
    assert_eq!(entry(menu(&menus, "File"), "file.open").1, None, "unbound");
    assert_eq!(
        entry(menu(&menus, "Layer"), "layer.delete").1,
        None,
        "unbound"
    );
    // A different registry gives different text: the hint is not typed here.
    let mac = menu_model::build_menus(&context(), &default_shortcuts_for(true));
    assert_eq!(entry(menu(&mac, "Edit"), "edit.undo").1, Some("Cmd+Z"));
    // Every hinted command is bound in the registry (no hint for a command
    // the table does not bind).
    let registry = default_shortcuts_for(false);
    for id in all_commands(&menus) {
        if let Some(app) = menu_model::hint_command(&id) {
            assert!(!registry.chords_for(&app).is_empty(), "{id}");
        }
    }
    assert_eq!(menu_model::hint_command("file.open"), None);
    assert_eq!(
        menu_model::hint_command("edit.undo"),
        Some(AppCommand::Undo)
    );
}

#[test]
fn widget_items_pair_every_row_with_its_command_and_show_the_hint() {
    let menus = menus_for(&context());
    let edit = menu(&menus, "Edit");
    let (items, commands) = menu_model::widget_items(edit);
    assert_eq!(items.len(), edit.entries.len());
    assert_eq!(commands.len(), items.len());
    assert_eq!(commands[0].as_deref(), Some("edit.undo"));
    assert!(items[0].label.starts_with("Undo"));
    assert!(items[0].label.ends_with("Ctrl+Z"));
    assert!(items[0].enabled);
    let separator = commands.iter().position(Option::is_none);
    assert_eq!(separator, Some(2));
    assert_eq!(
        items[2].kind,
        aurora_widgets::widgets::MenuItemKind::Separator
    );
    let mut off = context();
    off.can_undo = false;
    let disabled = menu_model::widget_items(menu(&menus_for(&off), "Edit")).0;
    assert!(!disabled[0].enabled);
}

#[test]
fn the_bar_shows_off_macos_unless_the_environment_says_otherwise() {
    use menu_model::menu_bar_enabled as on;
    assert!(on(false, None), "Linux and Windows show it");
    assert!(!on(true, None), "macOS keeps its native menu");
    assert!(on(true, Some("1")), "forced on");
    assert!(on(true, Some(" TRUE ")));
    assert!(!on(false, Some("0")), "forced off");
    assert!(!on(false, Some("off")));
    assert!(on(false, Some("")), "empty means detect");
    assert!(!on(true, Some("")));
    assert!(on(false, Some("maybe")), "unknown means detect");
}

#[test]
fn only_quit_runs_under_a_dialog() {
    assert!(crate::menu_command_allowed_with_dialog(
        crate::COMMAND_APP_QUIT
    ));
    for id in all_commands(&menus_for(&context())) {
        assert_eq!(
            crate::menu_command_allowed_with_dialog(&id),
            id == crate::COMMAND_APP_QUIT,
            "{id}"
        );
    }
}

#[test]
fn wrap_step_wraps_both_ways() {
    assert_eq!(wrap_step(5, 6, true), 0);
    assert_eq!(wrap_step(0, 6, false), 5);
    assert_eq!(wrap_step(2, 6, true), 3);
    assert_eq!(wrap_step(2, 6, false), 1);
    assert_eq!(wrap_step(3, 0, true), 0);
}

// -- the state machine, driven through the production routers --

struct Rig {
    ws: aurora_ui::Workspace,
    focus: FocusManager,
    scales: aurora_theme::Scales,
    state: MenuBarState,
    click: aurora_widgets::ClickTracker,
    clipboard: crate::tests::FakeClipboard,
}

impl Rig {
    fn new() -> Self {
        let scales = crate::test_workspace_scales();
        let mut ws = aurora_ui::build_workspace(&scales);
        let bar = ws.menu_bar;
        assert!(aurora_ui::set_menu_bar_shown(&mut ws.tree, &bar, true).is_ok());
        let mut rig = Self {
            ws,
            focus: FocusManager::default(),
            scales,
            state: MenuBarState::default(),
            click: aurora_widgets::ClickTracker::default(),
            clipboard: crate::tests::FakeClipboard::default(),
        };
        rig.relayout();
        rig
    }

    fn relayout(&mut self) {
        self.ws.tree.compute_layout(1280.0, 800.0);
    }

    fn open(&mut self, index: usize, ctx: &MenuContext) -> bool {
        let menus = menus_for(ctx);
        let opened = self.state.open(
            &mut self.ws,
            &mut self.focus,
            &self.scales,
            &menus,
            index,
            180.0,
            false,
        );
        self.relayout();
        opened
    }

    fn center(&self, id: WidgetId) -> (f32, f32) {
        let Some(b) = self.ws.tree.bounds(id) else {
            unreachable!("laid out");
        };
        #[allow(clippy::cast_precision_loss)]
        (
            b.x as f32 + b.width as f32 / 2.0,
            b.y as f32 + b.height as f32 / 2.0,
        )
    }

    /// One phase of a click on `id` through `route_widget_pointer`, then the
    /// bar's reaction to its outcome — as `App::route_gallery` does it, one
    /// phase at a time.
    fn phase(&mut self, id: WidgetId, phase: PointerPhase) -> BarAction {
        let at = self.center(id);
        let mut hit = crate::NoTextHit;
        let routed = crate::route_widget_pointer(
            &mut self.ws,
            &mut self.focus,
            &mut None,
            None,
            None,
            &mut self.click,
            &self.scales,
            false,
            phase,
            at,
            Modifiers::none(),
            &mut hit,
        );
        assert!(
            routed.owner == Some(crate::WidgetOwner::MenuBar) || routed.outcome.is_none(),
            "{:?}",
            routed.owner
        );
        match routed.outcome.as_ref() {
            Some(outcome) => self
                .state
                .action_for(&mut self.ws, &mut self.focus, outcome),
            None => BarAction::None,
        }
    }

    /// A full click: the first action either phase asked for.
    fn click_action(&mut self, id: WidgetId) -> BarAction {
        let down = self.phase(id, PointerPhase::Down);
        let up = self.phase(id, PointerPhase::Up);
        if down == BarAction::None { up } else { down }
    }

    /// A key through `route_widget_key`, then the bar's reaction.
    fn key(&mut self, named: NamedKey) -> (Option<KeyAction>, BarAction) {
        let bar_took = self.state.key(&mut self.ws, &mut self.focus, named);
        if bar_took != KeyAction::Pass {
            return (Some(bar_took), BarAction::None);
        }
        let routed = crate::route_widget_key(
            &mut self.ws,
            &mut self.focus,
            &mut None,
            None,
            None,
            &self.scales,
            false,
            false,
            Modifiers::none(),
            Key::Named(named),
            None,
            &mut self.clipboard,
        );
        let action = match routed {
            Some((crate::WidgetOwner::MenuBar, aurora_widgets::KeyOutcome::Handled(outcome))) => {
                self.state
                    .action_for(&mut self.ws, &mut self.focus, &outcome)
            }
            _ => BarAction::None,
        };
        (None, action)
    }

    fn popup_count(&self) -> usize {
        self.ws
            .tree
            .children(self.ws.menu_bar.root)
            .unwrap_or_default()
            .iter()
            .filter(|id| matches!(self.ws.tree.payload(**id), Some(WidgetKind::Menu(_))))
            .count()
    }

    fn expanded(&self) -> Vec<bool> {
        self.ws
            .menu_bar
            .buttons
            .iter()
            .map(|b| {
                self.ws
                    .tree
                    .accessibility(*b)
                    .and_then(accesskit::Node::is_expanded)
                    == Some(true)
            })
            .collect()
    }

    fn row_of(&self, command: &str, index: usize) -> WidgetId {
        let Some(open) = self.state.open.as_ref() else {
            unreachable!("a menu is open");
        };
        let _ = index;
        let Some(position) = open
            .commands
            .iter()
            .position(|c| c.as_deref() == Some(command))
        else {
            unreachable!("{command} is not in the open menu");
        };
        let Ok(state) = aurora_widgets::widgets::menu_state(&self.ws.tree, open.menu) else {
            unreachable!("open");
        };
        state.item_ids()[position]
    }
}

#[test]
fn clicking_a_title_routes_to_the_bar_and_toggles_its_menu() {
    let mut rig = Rig::new();
    let title = rig.ws.menu_bar.buttons[0];
    assert_eq!(rig.click_action(title), BarAction::Toggle(0));
    assert!(rig.open(0, &context()));
    assert_eq!(rig.popup_count(), 1);
    assert_eq!(rig.expanded(), [true, false, false, false, false, false]);
    let open = rig.state.open.clone();
    assert_eq!(open.as_ref().map(|o| o.index), Some(0));
    // The popup holds the focus (a screen reader follows the highlight).
    assert_eq!(rig.focus.focused(), open.map(|o| o.menu));
    // A second click on the same title asks to toggle: the app closes it.
    assert_eq!(rig.click_action(title), BarAction::Toggle(0));
    assert!(rig.state.close(&mut rig.ws, &mut rig.focus, true));
    assert_eq!(rig.popup_count(), 0);
    assert_eq!(rig.expanded(), [false; 6]);
    assert_eq!(
        rig.focus.focused(),
        Some(title),
        "focus returns to the title"
    );
}

#[test]
fn opening_a_second_menu_replaces_the_first_so_one_popup_exists() {
    let mut rig = Rig::new();
    assert!(rig.open(0, &context()));
    let first = rig.state.open.clone().map(|o| o.menu);
    assert!(rig.open(3, &context()));
    assert_eq!(rig.popup_count(), 1);
    assert_eq!(rig.expanded(), [false, false, false, true, false, false]);
    let second = rig.state.open.clone().map(|o| o.menu);
    assert_ne!(first, second);
    assert!(first.is_some_and(|menu| !rig.ws.tree.contains(menu)));
    // An out-of-range index opens nothing and leaves the open menu alone.
    assert!(!rig.open(9, &context()));
    assert_eq!(rig.popup_count(), 1);
}

#[test]
fn activating_an_enabled_row_runs_its_command_and_closes_the_menu() {
    let mut rig = Rig::new();
    assert!(rig.open(2, &context()));
    let row = rig.row_of("layer.new_group", 2);
    assert_eq!(
        rig.click_action(row),
        BarAction::Run("layer.new_group".to_owned())
    );
    assert_eq!(rig.popup_count(), 0);
    assert!(rig.state.open.is_none());
    assert_eq!(rig.expanded(), [false; 6]);
    assert_eq!(rig.focus.focused(), Some(rig.ws.menu_bar.buttons[2]));
}

#[test]
fn a_disabled_row_cannot_be_activated_and_leaves_the_menu_open() {
    let mut rig = Rig::new();
    let mut ctx = context();
    ctx.can_undo = false;
    ctx.can_redo = false;
    assert!(rig.open(1, &ctx));
    let undo = rig.row_of("edit.undo", 1);
    assert_eq!(rig.click_action(undo), BarAction::None);
    assert!(rig.state.open.is_some());
    assert_eq!(rig.popup_count(), 1);
    // The initial highlight skips the disabled Undo and Redo: it is on the
    // first enabled row, and Enter runs that one.
    let (_, action) = rig.key(NamedKey::Enter);
    assert_eq!(action, BarAction::Run("colour.swap".to_owned()));
}

#[test]
fn escape_closes_the_popup_and_returns_focus_to_the_title() {
    let mut rig = Rig::new();
    assert!(rig.open(4, &context()));
    let (took, action) = rig.key(NamedKey::Escape);
    assert_eq!(took, None);
    assert_eq!(action, BarAction::None);
    assert!(rig.state.open.is_none());
    assert_eq!(rig.popup_count(), 0);
    assert_eq!(rig.expanded(), [false; 6]);
    assert_eq!(rig.focus.focused(), Some(rig.ws.menu_bar.buttons[4]));
}

#[test]
fn down_and_enter_walk_the_popup_through_the_toolkits_keys() {
    let mut rig = Rig::new();
    assert!(rig.open(2, &context()));
    let (took, _) = rig.key(NamedKey::ArrowDown);
    assert_eq!(took, None, "Down is the menu's own key");
    let (_, action) = rig.key(NamedKey::Enter);
    assert_eq!(action, BarAction::Run("layer.new_group".to_owned()));
}

#[test]
fn left_and_right_move_between_open_menus_and_wrap() {
    let mut rig = Rig::new();
    assert!(rig.open(0, &context()));
    assert_eq!(rig.key(NamedKey::ArrowLeft).0, Some(KeyAction::Open(5)));
    assert_eq!(rig.key(NamedKey::ArrowRight).0, Some(KeyAction::Open(1)));
    assert!(rig.open(5, &context()));
    assert_eq!(rig.key(NamedKey::ArrowRight).0, Some(KeyAction::Open(0)));
    // Other keys are the toolkit's, not the bar's.
    assert_eq!(rig.key(NamedKey::Home).0, None);
}

#[test]
fn keys_on_a_focused_title_open_it_or_move_along_the_bar() {
    let mut rig = Rig::new();
    let titles = rig.ws.menu_bar.buttons;
    assert!(rig.focus.focus(&mut rig.ws.tree, titles[1]).is_ok());
    assert_eq!(rig.key(NamedKey::ArrowDown).0, Some(KeyAction::Open(1)));
    assert_eq!(rig.key(NamedKey::ArrowRight).0, Some(KeyAction::Handled));
    assert_eq!(rig.focus.focused(), Some(titles[2]));
    assert!(rig.focus.focus(&mut rig.ws.tree, titles[0]).is_ok());
    assert_eq!(rig.key(NamedKey::ArrowLeft).0, Some(KeyAction::Handled));
    assert_eq!(rig.focus.focused(), Some(titles[5]), "wraps");
    // Focus elsewhere: the bar passes every key.
    let canvas_button = rig.ws.tools.buttons[0].1;
    assert!(rig.focus.focus(&mut rig.ws.tree, canvas_button).is_ok());
    assert_eq!(rig.key(NamedKey::ArrowRight).0, None);
    // Enter on a focused title activates it (a toggle for the app).
    assert!(rig.focus.focus(&mut rig.ws.tree, titles[3]).is_ok());
    assert_eq!(rig.key(NamedKey::Enter).1, BarAction::Toggle(3));
}

#[test]
fn tab_closes_an_open_menu_and_is_passed_on() {
    let mut rig = Rig::new();
    assert!(rig.open(1, &context()));
    assert_eq!(rig.key(NamedKey::Tab).0, None);
    assert!(rig.state.open.is_none());
    assert_eq!(rig.popup_count(), 0);
}

#[test]
fn a_press_outside_closes_the_menu_but_not_on_a_title_or_inside_it() {
    let mut rig = Rig::new();
    assert!(rig.open(0, &context()));
    let menu = rig.state.open.clone().map(|o| o.menu);
    let Some(menu) = menu else {
        unreachable!("open")
    };
    // Inside the popup: stays.
    let inside = rig.center(menu);
    assert!(!rig.state.light_dismiss(&mut rig.ws, &mut rig.focus, inside));
    // On a title (any): stays — the click decides.
    let title = rig.center(rig.ws.menu_bar.buttons[3]);
    assert!(!rig.state.light_dismiss(&mut rig.ws, &mut rig.focus, title));
    assert!(rig.state.open.is_some());
    // On the canvas area: closes, and focus returns to the title.
    let canvas = rig.center(rig.ws.canvas_area);
    assert!(rig.state.light_dismiss(&mut rig.ws, &mut rig.focus, canvas));
    assert!(rig.state.open.is_none());
    assert_eq!(rig.popup_count(), 0);
    assert_eq!(rig.focus.focused(), Some(rig.ws.menu_bar.buttons[0]));
    // Nothing open: nothing to dismiss.
    assert!(!rig.state.light_dismiss(&mut rig.ws, &mut rig.focus, canvas));
}

#[test]
fn hovering_another_title_switches_only_while_a_menu_is_open() {
    let mut rig = Rig::new();
    let titles = rig.ws.menu_bar.buttons;
    assert_eq!(
        rig.state.hover_switch_target(&rig.ws, Some(titles[2])),
        None
    );
    assert!(rig.open(0, &context()));
    assert_eq!(
        rig.state.hover_switch_target(&rig.ws, Some(titles[2])),
        Some(2)
    );
    assert_eq!(
        rig.state.hover_switch_target(&rig.ws, Some(titles[0])),
        None,
        "the open title itself"
    );
    assert_eq!(
        rig.state
            .hover_switch_target(&rig.ws, Some(rig.ws.canvas_area)),
        None
    );
    assert_eq!(rig.state.hover_switch_target(&rig.ws, None), None);
}

#[test]
fn no_menu_opens_under_a_modal_and_a_modal_closes_an_open_one() {
    let mut rig = Rig::new();
    let menus = menus_for(&context());
    assert!(!rig.state.open(
        &mut rig.ws,
        &mut rig.focus,
        &rig.scales,
        &menus,
        0,
        100.0,
        true
    ));
    assert_eq!(rig.popup_count(), 0);
    assert!(rig.open(0, &context()));
    assert!(
        !rig.state
            .close_for_modal(&mut rig.ws, &mut rig.focus, false)
    );
    assert!(rig.state.close_for_modal(&mut rig.ws, &mut rig.focus, true));
    assert_eq!(rig.popup_count(), 0);
    assert_eq!(rig.expanded(), [false; 6]);
}

#[test]
fn a_hidden_bar_opens_nothing() {
    let mut rig = Rig::new();
    let bar = rig.ws.menu_bar;
    assert!(aurora_ui::set_menu_bar_shown(&mut rig.ws.tree, &bar, false).is_ok());
    rig.relayout();
    assert!(!rig.open(0, &context()));
}

#[test]
fn a_popup_removed_behind_the_states_back_is_forgotten() {
    let mut rig = Rig::new();
    assert!(rig.open(0, &context()));
    let Some(menu) = rig.state.open.clone().map(|o| o.menu) else {
        unreachable!("open")
    };
    assert!(rig.ws.tree.remove(menu).is_ok());
    rig.state.forget_stale(&mut rig.ws);
    assert!(rig.state.open.is_none());
    assert_eq!(rig.expanded(), [false; 6]);
}

#[test]
fn widget_owner_claims_titles_and_popup_rows_for_the_bar_only() {
    let mut rig = Rig::new();
    assert!(rig.open(0, &context()));
    let owner = |rig: &Rig, id| crate::widget_owner(&rig.ws.tree, None, None, None, id);
    for title in rig.ws.menu_bar.buttons {
        assert_eq!(owner(&rig, title), Some(crate::WidgetOwner::MenuBar));
    }
    let row = rig.row_of("file.open", 0);
    assert_eq!(owner(&rig, row), Some(crate::WidgetOwner::MenuBar));
    assert_eq!(owner(&rig, rig.ws.options_bar), None);
    assert_ne!(
        owner(&rig, rig.ws.tools.buttons[0].1),
        Some(crate::WidgetOwner::MenuBar)
    );
}

#[test]
fn the_popup_is_a_menu_whose_rows_are_menu_items_with_disabled_state() {
    let mut rig = Rig::new();
    let mut ctx = context();
    ctx.can_redo = false;
    assert!(rig.open(1, &ctx));
    let Some(open) = rig.state.open.clone() else {
        unreachable!("open")
    };
    let Some(node) = rig.ws.tree.accessibility(open.menu) else {
        unreachable!("built");
    };
    assert_eq!(node.role(), accesskit::Role::Menu);
    assert_eq!(node.label(), Some("Edit"));
    let redo = rig.row_of("edit.redo", 1);
    let undo = rig.row_of("edit.undo", 1);
    let node = |id| rig.ws.tree.accessibility(id).cloned();
    assert!(node(redo).is_some_and(|n| n.role() == accesskit::Role::MenuItem && n.is_disabled()));
    assert!(node(undo).is_some_and(|n| n.role() == accesskit::Role::MenuItem && !n.is_disabled()));
    // The title announces the open state.
    let title = node(rig.ws.menu_bar.buttons[1]);
    assert_eq!(
        title.as_ref().and_then(accesskit::Node::is_expanded),
        Some(true)
    );
    assert_eq!(title.map(|n| n.role()), Some(accesskit::Role::Button));
}

#[test]
fn a_press_on_the_popup_is_not_a_canvas_press() {
    // The popup is a popover: a hit inside it reports the menu's own rows,
    // not the canvas under it, so the canvas gets no pointer event there.
    let mut rig = Rig::new();
    assert!(rig.open(0, &context()));
    let row = rig.row_of("file.open", 0);
    let at = rig.center(row);
    let hit = rig.ws.tree.hit_test(at);
    assert_eq!(hit, Some(row));
    // Outside the popup the canvas is hit as usual.
    let canvas = rig.center(rig.ws.canvas_area);
    assert!(rig.ws.tree.hit_test(canvas).is_some_and(|id| id != row));
}

#[test]
fn an_activated_outcome_for_a_foreign_menu_is_ignored() {
    let mut rig = Rig::new();
    assert!(rig.open(0, &context()));
    let stranger = rig.ws.canvas_area;
    let outcome = PointerOutcome::Action(ActionOutcome::MenuActivated {
        menu: stranger,
        index: 0,
    });
    assert_eq!(
        rig.state.action_for(&mut rig.ws, &mut rig.focus, &outcome),
        BarAction::None
    );
    assert!(rig.state.open.is_some(), "the real menu stays open");
}

fn at_click(rig: &mut Rig, id: WidgetId) -> crate::AccessibilityReaction {
    let request = accesskit::ActionRequest {
        action: accesskit::Action::Click,
        target_tree: aurora_widgets::ACCESSIBILITY_TREE_ID,
        target_node: id,
        data: None,
    };
    crate::route_accessibility_action(
        &mut rig.ws,
        &mut rig.focus,
        None,
        &std::collections::HashMap::new(),
        None,
        None,
        &request,
    )
}

#[test]
fn an_assistive_click_on_a_title_or_a_popup_item_reaches_the_bar() {
    let mut rig = Rig::new();
    let title = rig.ws.menu_bar.buttons[0];
    let reaction = at_click(&mut rig, title);
    assert!(
        matches!(
            reaction,
            crate::AccessibilityReaction::MenuBar(ActionOutcome::Activated(id)) if id == title
        ),
        "{reaction:?}"
    );
    assert!(rig.open(0, &context()));
    let row = rig.row_of("file.open", 0);
    let reaction = at_click(&mut rig, row);
    let crate::AccessibilityReaction::MenuBar(outcome) = reaction else {
        unreachable!("{reaction:?}");
    };
    let action = rig.state.action_for(
        &mut rig.ws,
        &mut rig.focus,
        &PointerOutcome::Action(outcome),
    );
    assert_eq!(action, BarAction::Run("file.open".to_owned()));
    assert_eq!(rig.popup_count(), 0);
    // A disabled row declares no Click: the request never reaches the bar.
    let mut ctx = context();
    ctx.can_undo = false;
    assert!(rig.open(1, &ctx));
    let undo = rig.row_of("edit.undo", 1);
    let reaction = at_click(&mut rig, undo);
    assert!(!matches!(
        reaction,
        crate::AccessibilityReaction::MenuBar(_)
    ));
    assert_eq!(rig.popup_count(), 1);
}
