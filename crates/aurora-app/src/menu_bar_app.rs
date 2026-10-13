//! `App`'s half of the in-window menu bar (0.225.0): gathering the facts
//! the menus depend on, opening a popup, and turning bar outcomes and keys
//! into commands. The state machine is [`crate::menu_bar_state`], the data
//! [`crate::menu_model`]; this file is only the glue that needs `App`'s
//! fields (so it is covered by the headless tests of those two, and by
//! reading — no window is built in tests).

use aurora_widgets::PointerOutcome;
use aurora_widgets::shortcut::NamedKey;

use crate::menu_bar_state::{BarAction, KeyAction};
use crate::menu_model::{self, MenuContext};
use crate::{App, Key, activate_command, menu_command_allowed_with_dialog};

impl App {
    /// A dialog or the command palette is open.
    fn menu_bar_modal(&self) -> bool {
        self.dialog.is_some() || self.command_palette.is_some()
    }

    /// The facts the menus' availability and dynamic sections come from.
    pub(crate) fn menu_context(&self) -> MenuContext {
        let active = self.doc.active_layer;
        MenuContext {
            can_undo: !self.doc.undo_order.undo.is_empty(),
            can_redo: !self.doc.undo_order.redo.is_empty(),
            delete_allowed: crate::delete_allowed(&self.doc.layers, active),
            quick_mask: self.quick_mask.is_some(),
            clipped: active.is_some_and(|layer| self.doc.layers.clipped(layer)),
            clip_refused: active
                .is_none_or(|layer| crate::clip_refusal(&self.doc.layers, layer).is_some()),
            workspaces: crate::workspace_presets::workspace_palette_entries(&self.presets),
            documents: crate::document_tabs::document_palette_entries(&self.shelf, &self.doc),
            saved_selections: crate::selection_store::palette_entries(&self.doc.selection),
        }
    }

    /// Opens menu `index` (closing any open one and the other popovers).
    pub(crate) fn open_bar_menu(&mut self, index: usize) {
        if self.menu_bar_modal() {
            return;
        }
        // One popover at a time: the colour picker and the blend-mode list
        // (and the gallery's) close first.
        self.close_colour_popover_now(false);
        let controls = self.layer_controls.controls;
        crate::close_open_popovers(
            &mut self.workspace.tree,
            &mut self.focus,
            self.gallery.as_mut(),
            controls.as_ref(),
            &mut self.scroll_follow,
        );
        let context = self.menu_context();
        let menus = menu_model::build_menus(&context, &self.shortcuts);
        let Some(menu) = menus.get(index) else {
            return;
        };
        let scale_factor = self.scale_factor;
        let scales = &self.scales;
        let mut engine = self.text_engine.as_mut();
        #[allow(clippy::cast_possible_truncation)]
        let mut measure = |text: &str| {
            engine.as_mut().map(|engine| {
                engine
                    .shape(
                        text,
                        &aurora_widgets::label_style(scales),
                        scale_factor as f32,
                    )
                    .width
            })
        };
        let widest = crate::menu_bar_state::widest_label(menu, scales, &mut measure);
        let modal = self.dialog.is_some() || self.command_palette.is_some();
        let opened = self.menu_bar.open(
            &mut self.workspace,
            &mut self.focus,
            &self.scales,
            &menus,
            index,
            widest,
            modal,
        );
        if opened {
            self.relayout_after_gallery();
        }
    }

    /// A title was activated: opens its menu, or closes it when open.
    fn toggle_bar_menu(&mut self, index: usize) {
        if self
            .menu_bar
            .open
            .as_ref()
            .is_some_and(|open| open.index == index)
        {
            self.menu_bar
                .close(&mut self.workspace, &mut self.focus, true);
            self.relayout_after_gallery();
        } else {
            self.open_bar_menu(index);
        }
    }

    /// What a routed outcome on the bar means (opening, closing, running).
    pub(crate) fn apply_menu_bar_outcome(&mut self, outcome: &PointerOutcome) {
        let action = self
            .menu_bar
            .action_for(&mut self.workspace, &mut self.focus, outcome);
        match action {
            BarAction::None => {}
            BarAction::Toggle(index) => {
                self.toggle_bar_menu(index);
                return;
            }
            BarAction::Run(command) => self.run_bar_command(&command),
        }
        self.relayout_after_gallery();
    }

    /// Runs a menu command through the same [`activate_command`] the native
    /// menu and the palette use, under the same dialog rule: while a modal
    /// dialog is open only [`menu_command_allowed_with_dialog`] commands run.
    pub(crate) fn run_bar_command(&mut self, id: &str) {
        if self.dialog.is_some() && !menu_command_allowed_with_dialog(id) {
            tracing::debug!(id, "ignoring a menu bar activation while a dialog is open");
            return;
        }
        if self.command_palette.is_some() {
            return;
        }
        let picked = activate_command(
            &mut self.workspace,
            &mut self.focus,
            id,
            &mut self.file_dialog,
        );
        self.run_activated(picked);
        self.push_accessibility();
    }

    /// The bar's keys; `true` when it took the key.
    pub(crate) fn menu_bar_key(&mut self, key: Key) -> bool {
        if self.menu_bar_modal() {
            return false;
        }
        let Key::Named(named) = key else {
            // A plain letter does not reach the tool shortcuts under an open
            // menu (a chord still does: Ctrl+Z is the app's).
            return self.menu_bar.open.is_some()
                && !aurora_widgets::is_shortcut_chord(self.modifiers);
        };
        if !matches!(
            named,
            NamedKey::ArrowLeft
                | NamedKey::ArrowRight
                | NamedKey::ArrowUp
                | NamedKey::ArrowDown
                | NamedKey::Tab
        ) {
            return false;
        }
        match self
            .menu_bar
            .key(&mut self.workspace, &mut self.focus, named)
        {
            KeyAction::Pass => false,
            KeyAction::Handled => {
                self.relayout_after_gallery();
                true
            }
            KeyAction::Open(index) => {
                self.open_bar_menu(index);
                true
            }
        }
    }

    /// A press outside the open popup closes it.
    pub(crate) fn light_dismiss_menu_bar(&mut self, position: (f32, f32)) {
        if self
            .menu_bar
            .light_dismiss(&mut self.workspace, &mut self.focus, position)
        {
            self.relayout_after_gallery();
        }
    }

    /// Tints the title under the pointer and, while a menu is open, switches
    /// to the title the pointer reaches.
    pub(crate) fn update_menu_bar_hover(&mut self, position: Option<(f32, f32)>) {
        if !aurora_ui::menu_bar_shown(&self.workspace.tree, &self.workspace.menu_bar) {
            return;
        }
        let busy = self.menu_bar_modal() || self.drag.is_some() || self.rail_resize.is_some();
        let under = position
            .filter(|_| !busy)
            .and_then(|point| self.workspace.tree.hit_test(point));
        let title = under.filter(|id| self.workspace.menu_bar.index_of(*id).is_some());
        let bar = self.workspace.menu_bar;
        match aurora_ui::set_menu_bar_hover(&mut self.workspace.tree, &bar, title) {
            Ok(true) => self.needs_redraw = true,
            Ok(false) => {}
            Err(err) => tracing::warn!(?err, "menu bar hover failed"),
        }
        if let Some(index) = self.menu_bar.hover_switch_target(&self.workspace, title) {
            self.open_bar_menu(index);
        }
    }

    /// Per event-loop turn: forgets a popup that went away, and closes an
    /// open menu under a modal.
    pub(crate) fn sync_menu_bar_now(&mut self) {
        self.menu_bar.forget_stale(&mut self.workspace);
        let modal = self.menu_bar_modal();
        if self
            .menu_bar
            .close_for_modal(&mut self.workspace, &mut self.focus, modal)
        {
            self.relayout_after_gallery();
        }
    }
}
