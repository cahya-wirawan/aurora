//! The in-window menu bar's live state and its free-function transitions
//! (0.225.0): which menu is open, opening, switching, closing, the keys
//! and the light dismiss. Like `colour_state`, none of it needs a window,
//! so each is tested on a headless workspace; `App` only calls it.
//!
//! **One menu at a time.** Opening one closes any other first; a click on
//! the open menu's title closes it; a hover over another title while a
//! menu is open switches to that one; `Left`/`Right` do the same from the
//! keyboard (wrapping); `Escape` (the toolkit's `MenuCancelled`) closes it
//! and focus returns to its title; a press anywhere else closes it
//! ([`light_dismiss`]). A press on the canvas still reaches the canvas, as
//! for the other popovers. A modal (a dialog or the command palette)
//! closes an open menu ([`close_for_modal`]) and no menu opens under one.
//!
//! Alt+letter mnemonics are not implemented.

use aurora_theme::Scales;
use aurora_ui::{MenuBar, Workspace};
use aurora_widgets::shortcut::NamedKey;
use aurora_widgets::{ActionOutcome, FocusManager, FocusOrigin, PointerOutcome, WidgetId};

use crate::menu_model::{self, Menu};

/// The open menu: its title index, its popup and the command each popup
/// row runs (`None` for a separator), parallel to the popup's items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenMenu {
    pub(crate) index: usize,
    pub(crate) menu: WidgetId,
    pub(crate) commands: Vec<Option<String>>,
}

/// The bar's state.
#[derive(Debug, Default)]
pub(crate) struct MenuBarState {
    pub(crate) open: Option<OpenMenu>,
}

/// What an outcome on the bar asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BarAction {
    /// Nothing for the app.
    None,
    /// A title was clicked or activated: open it, or close it if open.
    Toggle(usize),
    /// An enabled item was activated: run this command (the popup is gone).
    Run(String),
}

/// What a key meant to the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyAction {
    /// Not the bar's.
    Pass,
    /// The bar took it and nothing else is needed.
    Handled,
    /// Open (or switch to) this menu.
    Open(usize),
}

impl MenuBarState {
    /// Forgets an open menu whose popup is no longer in the tree.
    pub(crate) fn forget_stale(&mut self, workspace: &mut Workspace) {
        if self
            .open
            .as_ref()
            .is_some_and(|open| !workspace.tree.contains(open.menu))
        {
            self.open = None;
            sync(workspace, None);
        }
    }

    /// Closes the open menu, if any. With `restore_focus`, focus that was
    /// inside the popup (or on nothing) goes back to the title. Returns
    /// whether anything closed.
    pub(crate) fn close(
        &mut self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        restore_focus: bool,
    ) -> bool {
        let Some(open) = self.open.take() else {
            return false;
        };
        let focus_inside = focus
            .focused()
            .is_none_or(|focused| workspace.tree.is_within(open.menu, focused));
        if workspace.tree.contains(open.menu)
            && let Err(err) = aurora_widgets::widgets::close_menu(&mut workspace.tree, open.menu)
        {
            tracing::warn!(?err, "failed to close the menu bar popup");
        }
        sync(workspace, None);
        focus.validate(&workspace.tree);
        if restore_focus
            && focus_inside
            && let Some(button) = workspace.menu_bar.buttons.get(open.index).copied()
            && let Err(err) = focus.focus_with(&mut workspace.tree, button, FocusOrigin::Keyboard)
        {
            tracing::debug!(?err, "the menu title is not focusable");
        }
        true
    }

    /// Opens menu `index` from `menus` (closing any other first) with the
    /// popup `widest_text` logical px of text wide. Refused (returns
    /// `false`) while `modal_open`, for an index past the menus, for a menu
    /// with no item at all, or when the bar is hidden.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open(
        &mut self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        scales: &Scales,
        menus: &[Menu],
        index: usize,
        widest_text: f32,
        modal_open: bool,
    ) -> bool {
        self.forget_stale(workspace);
        if modal_open || !aurora_ui::menu_bar_shown(&workspace.tree, &workspace.menu_bar) {
            return false;
        }
        let Some(menu) = menus.get(index) else {
            return false;
        };
        self.close(workspace, focus, false);
        let (items, commands) = menu_model::widget_items(menu);
        let width = aurora_ui::menu_popup_width(scales, widest_text);
        let bar = workspace.menu_bar;
        let popup = match aurora_ui::open_menu_under(
            &mut workspace.tree,
            &bar,
            scales,
            index,
            width,
            items,
        ) {
            Ok(popup) => popup,
            Err(err) => {
                tracing::warn!(?err, "failed to open a menu bar popup");
                return false;
            }
        };
        if let Err(err) = focus.focus_with(&mut workspace.tree, popup, FocusOrigin::Keyboard) {
            tracing::warn!(?err, "failed to focus the menu bar popup");
        }
        self.open = Some(OpenMenu {
            index,
            menu: popup,
            commands,
        });
        sync(workspace, Some(index));
        true
    }

    /// Closes the menu when a dialog or the palette is open.
    pub(crate) fn close_for_modal(
        &mut self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        modal_open: bool,
    ) -> bool {
        modal_open && self.close(workspace, focus, false)
    }

    /// Light dismiss for a primary `Down` at `point`: a press outside the
    /// popup and off every title closes it. A press on a title is left to
    /// the click (which toggles). Focus that was inside returns to the
    /// title. Returns whether it closed.
    pub(crate) fn light_dismiss(
        &mut self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        point: (f32, f32),
    ) -> bool {
        self.forget_stale(workspace);
        let Some(open) = self.open.as_ref() else {
            return false;
        };
        let hit = workspace.tree.hit_test(point);
        let inside = hit.is_some_and(|hit| {
            workspace.tree.is_within(open.menu, hit) || workspace.menu_bar.index_of(hit).is_some()
        });
        !inside && self.close(workspace, focus, true)
    }

    /// The menu whose title is under `hit`, when another menu is open (the
    /// hover-switch rule); `None` otherwise.
    pub(crate) fn hover_switch_target(
        &self,
        workspace: &Workspace,
        hit: Option<WidgetId>,
    ) -> Option<usize> {
        let open = self.open.as_ref()?;
        let index = workspace.menu_bar.index_of(hit?)?;
        (index != open.index).then_some(index)
    }

    /// What a routed outcome on the bar means.
    pub(crate) fn action_for(
        &mut self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        outcome: &PointerOutcome,
    ) -> BarAction {
        self.forget_stale_after(workspace, outcome);
        match outcome {
            PointerOutcome::Action(ActionOutcome::Activated(id)) => {
                match workspace.menu_bar.index_of(*id) {
                    Some(index) => BarAction::Toggle(index),
                    None => BarAction::None,
                }
            }
            PointerOutcome::Action(ActionOutcome::MenuActivated { menu, index }) => {
                let Some(open) = self.open.take_if(|open| open.menu == *menu) else {
                    return BarAction::None;
                };
                // The toolkit already removed the popup.
                sync(workspace, None);
                focus.validate(&workspace.tree);
                if let Some(button) = workspace.menu_bar.buttons.get(open.index).copied() {
                    let _ = focus.focus_with(&mut workspace.tree, button, FocusOrigin::Keyboard);
                }
                match open.commands.get(*index) {
                    Some(Some(command)) => BarAction::Run(command.clone()),
                    _ => BarAction::None,
                }
            }
            PointerOutcome::MenuCancelled { menu } => {
                if self.open.as_ref().is_some_and(|open| open.menu == *menu) {
                    // The toolkit removed the popup: forget it, restore focus.
                    if let Some(open) = self.open.take() {
                        sync(workspace, None);
                        focus.validate(&workspace.tree);
                        if let Some(button) = workspace.menu_bar.buttons.get(open.index).copied() {
                            let _ = focus.focus_with(
                                &mut workspace.tree,
                                button,
                                FocusOrigin::Keyboard,
                            );
                        }
                    }
                }
                BarAction::None
            }
            _ => BarAction::None,
        }
    }

    fn forget_stale_after(&mut self, workspace: &mut Workspace, outcome: &PointerOutcome) {
        // A popup the toolkit removed on its own (an item activated, `Escape`)
        // is handled by the arms above; any other outcome with a missing
        // popup is a stale record.
        let handled = matches!(
            outcome,
            PointerOutcome::Action(ActionOutcome::MenuActivated { .. })
                | PointerOutcome::MenuCancelled { .. }
        );
        if !handled {
            self.forget_stale(workspace);
        }
    }

    /// A key the bar may take. `Left`/`Right` move between menus while one
    /// is open (wrapping) and between titles while focus rests on one;
    /// `Down`/`Up` on a focused title open it; `Tab` closes an open menu
    /// and is passed on.
    pub(crate) fn key(
        &mut self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        key: NamedKey,
    ) -> KeyAction {
        self.forget_stale(workspace);
        let count = workspace.menu_bar.buttons.len();
        if let Some(open) = self.open.as_ref() {
            let in_popup = focus
                .focused()
                .is_some_and(|focused| workspace.tree.is_within(open.menu, focused));
            return match key {
                NamedKey::ArrowLeft if in_popup => {
                    KeyAction::Open(wrap_step(open.index, count, false))
                }
                NamedKey::ArrowRight if in_popup => {
                    KeyAction::Open(wrap_step(open.index, count, true))
                }
                NamedKey::Tab => {
                    self.close(workspace, focus, false);
                    KeyAction::Pass
                }
                _ => KeyAction::Pass,
            };
        }
        let Some(index) = focus
            .focused()
            .and_then(|focused| workspace.menu_bar.index_of(focused))
        else {
            return KeyAction::Pass;
        };
        match key {
            NamedKey::ArrowDown | NamedKey::ArrowUp => KeyAction::Open(index),
            NamedKey::ArrowLeft | NamedKey::ArrowRight => {
                let next = wrap_step(index, count, key == NamedKey::ArrowRight);
                if let Some(button) = workspace.menu_bar.buttons.get(next).copied() {
                    let _ = focus.focus_with(&mut workspace.tree, button, FocusOrigin::Keyboard);
                }
                KeyAction::Handled
            }
            _ => KeyAction::Pass,
        }
    }
}

/// `index` one step forward or back among `count`, wrapping.
pub(crate) fn wrap_step(index: usize, count: usize, forward: bool) -> usize {
    if count == 0 {
        return 0;
    }
    if forward {
        (index + 1) % count
    } else {
        (index + count - 1) % count
    }
}

/// Mirrors the open menu into the titles (`expanded`).
fn sync(workspace: &mut Workspace, open: Option<usize>) {
    let bar: MenuBar = workspace.menu_bar;
    if let Err(err) = aurora_ui::sync_menu_bar_open(&mut workspace.tree, &bar, open) {
        tracing::warn!(?err, "failed to sync the menu bar titles");
    }
}

/// The widest text, in logical px, among a menu's visible item labels, for
/// the popup width: measured by `measure` (the text engine's shaped width)
/// or, when no engine exists yet, estimated from the character count.
pub(crate) fn widest_label(
    menu: &Menu,
    scales: &Scales,
    measure: &mut dyn FnMut(&str) -> Option<f32>,
) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let estimate =
        |text: &str| text.chars().count() as f32 * scales.typography.size.md as f32 * 0.6;
    menu.entries
        .iter()
        .filter_map(|entry| match entry {
            menu_model::Entry::Item { label, hint, .. } => {
                Some(menu_model::display_label(label, hint.as_deref()))
            }
            menu_model::Entry::Separator => None,
        })
        .map(|text| {
            measure(&text)
                .filter(|w| w.is_finite())
                .unwrap_or_else(|| estimate(&text))
        })
        .fold(0.0_f32, f32::max)
}
