//! The real in-window menus (the real `menu_model`, 0.259.0) in windows of
//! several sizes: every popup stays inside its window, rows keep one
//! pitch, the keyboard and the wheel reach the last item, and a click
//! after scrolling runs the command under the pointer.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use aurora_widgets::shortcut::{Key, Modifiers, NamedKey};
use aurora_widgets::widgets::{self, CommandEntry, MenuItemKind};
use aurora_widgets::{FocusManager, PointerPhase, WidgetId};

use crate::menu_bar_state::{BarAction, MenuBarState};
use crate::menu_model::{self, Menu, MenuContext};
use crate::{default_shortcuts_for, scroll_panel};

/// Window sizes in logical px: the default window, a small one, the
/// minimum window, and the minimum window's size at scale factor 2.
const SIZES: [(f32, f32); 4] = [
    (1274.0, 900.0),
    (800.0, 600.0),
    (640.0, 480.0),
    (637.0, 450.0),
];

fn context(saved: usize) -> MenuContext {
    let mut saved_selections = Vec::new();
    for index in 0..saved {
        saved_selections.push(CommandEntry::new(
            format!("select.load:s{index}"),
            format!("Load Selection: s{index}"),
        ));
        saved_selections.push(CommandEntry::new(
            format!("select.delete_saved:s{index}"),
            format!("Delete Saved Selection: s{index}"),
        ));
    }
    MenuContext {
        can_undo: true,
        can_redo: false,
        delete_allowed: true,
        quick_mask: false,
        clipped: false,
        clip_refused: false,
        workspaces: crate::workspace_presets::workspace_palette_entries(
            &crate::workspace_presets::WorkspacePresets::default(),
        ),
        documents: vec![CommandEntry::new("document.switch.7", "Document: a.png")],
        saved_selections,
    }
}

struct Rig {
    ws: aurora_ui::Workspace,
    focus: FocusManager,
    scales: aurora_theme::Scales,
    state: MenuBarState,
    click: aurora_widgets::ClickTracker,
    clipboard: crate::tests::FakeClipboard,
    size: (f32, f32),
}

impl Rig {
    fn new(size: (f32, f32)) -> Self {
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
            size,
        };
        rig.relayout();
        rig
    }

    fn relayout(&mut self) {
        aurora_widgets::compute_text_layout(&mut self.ws.tree, self.size.0, self.size.1, None);
    }

    fn open(&mut self, index: usize, ctx: &MenuContext) -> Vec<Menu> {
        let menus = menu_model::build_menus(ctx, &default_shortcuts_for(false));
        assert!(self.state.open(
            &mut self.ws,
            &mut self.focus,
            &self.scales,
            &menus,
            index,
            180.0,
            false,
        ));
        self.relayout();
        menus
    }

    fn popup(&self) -> WidgetId {
        match self.state.open.as_ref() {
            Some(open) => open.menu,
            None => unreachable!("a menu is open"),
        }
    }

    fn menu_state(&self) -> &widgets::MenuState {
        match widgets::menu_state(&self.ws.tree, self.popup()) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn body(&self) -> WidgetId {
        match self.menu_state().body() {
            Some(body) => body,
            None => unreachable!("scrolling body"),
        }
    }

    fn bounds(&self, id: WidgetId) -> aurora_core::Rect {
        match self.ws.tree.bounds(id) {
            Some(bounds) => bounds,
            None => unreachable!("laid out"),
        }
    }

    fn key(&mut self, named: NamedKey) -> BarAction {
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
        self.relayout();
        action
    }

    fn click_at(&mut self, at: (f32, f32)) -> BarAction {
        let mut result = BarAction::None;
        for phase in [PointerPhase::Down, PointerPhase::Up] {
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
            if let Some(outcome) = routed.outcome.as_ref() {
                let action = self
                    .state
                    .action_for(&mut self.ws, &mut self.focus, outcome);
                if action != BarAction::None {
                    result = action;
                }
            }
        }
        result
    }

    /// One wheel notch down at `at`, through the production routing.
    fn wheel(&mut self, at: (f32, f32), lines: f32, scale_factor: f64) -> bool {
        let Some(list) = widgets::popup_scroll_target(&self.ws.tree, at) else {
            return false;
        };
        scroll_panel(
            &mut self.ws.tree,
            list,
            winit::event::MouseScrollDelta::LineDelta(0.0, -lines),
            scale_factor,
            &self.scales,
        )
    }

    fn highlight_in_view(&self) -> bool {
        let state = self.menu_state();
        let Some(row) = state.highlighted().and_then(|i| state.item_ids().get(i)) else {
            return false;
        };
        let (row, view) = (self.bounds(*row), self.bounds(self.body()));
        row.y >= view.y && row.bottom() <= view.bottom()
    }
}

fn row_pitch(rig: &Rig) -> (i64, i64) {
    let scales = &rig.scales;
    (
        widgets::row_height(scales) as i64,
        i64::from(scales.spacing.xxs),
    )
}

#[test]
fn every_real_menu_fits_inside_every_window_with_64_saved_selections() {
    for size in SIZES {
        for index in 0..6 {
            let mut rig = Rig::new(size);
            let menus = rig.open(index, &context(64));
            let frame = rig.bounds(rig.popup());
            assert!(
                frame.x >= 0 && frame.right() <= size.0 as i64,
                "{}",
                menus[index].title
            );
            assert!(
                frame.y >= 0 && frame.bottom() <= size.1 as i64,
                "{} at {size:?}: {frame:?}",
                menus[index].title
            );
        }
    }
}

#[test]
fn the_select_menu_with_saved_selections_overflows_and_gets_a_scrollbar() {
    let mut rig = Rig::new((640.0, 480.0));
    rig.open(3, &context(64));
    let body = rig.body();
    assert!(rig.ws.tree.scroll_range(body).unwrap_or(0.0) > 0.0);
    let bar = rig.menu_state().scrollbar();
    assert_eq!(
        bar.and_then(|b| rig.ws.tree.style(b)).map(|s| s.display),
        Some(taffy::Display::Flex)
    );
}

#[test]
fn rows_never_overlap_in_any_real_menu_at_any_size_or_scroll() {
    for size in SIZES {
        for index in 0..6 {
            let mut rig = Rig::new(size);
            rig.open(index, &context(64));
            let (row, sep) = row_pitch(&rig);
            let max = rig.ws.tree.scroll_range(rig.body()).unwrap_or(0.0);
            for offset in [0.0, max / 2.0, max] {
                let _ = rig.ws.tree.set_scroll_y(rig.body(), offset);
                let items = rig.menu_state().items().to_vec();
                let ids = rig.menu_state().item_ids().to_vec();
                for i in 0..ids.len() - 1 {
                    let height = match items[i].kind {
                        MenuItemKind::Action => row,
                        MenuItemKind::Separator => sep,
                    };
                    let (a, b) = (rig.bounds(ids[i]), rig.bounds(ids[i + 1]));
                    assert_eq!(b.y - a.y, height, "menu {index} item {i} at {size:?}");
                    assert_eq!(i64::from(a.height), height);
                }
            }
        }
    }
}

#[test]
fn end_and_up_reach_the_last_command_of_every_menu_and_enter_runs_it() {
    for size in SIZES {
        for index in 0..6 {
            let mut rig = Rig::new(size);
            rig.open(index, &context(64));
            let last = {
                let open = rig.state.open.as_ref();
                let commands = open.map(|o| o.commands.clone()).unwrap_or_default();
                let items = rig.menu_state().items().to_vec();
                let position = (0..items.len()).rev().find(|&i| {
                    items[i].kind == MenuItemKind::Action
                        && items[i].enabled
                        && commands[i].is_some()
                });
                position.and_then(|i| commands[i].clone())
            };
            let Some(last) = last else {
                unreachable!("every menu has an enabled item");
            };
            // ArrowUp from the first item wraps to the last.
            rig.key(NamedKey::ArrowUp);
            assert!(rig.highlight_in_view(), "menu {index} at {size:?}");
            let action = rig.key(NamedKey::Enter);
            assert_eq!(action, BarAction::Run(last), "menu {index} at {size:?}");
        }
    }
}

#[test]
fn down_never_lands_on_a_separator_or_a_disabled_item_while_scrolling() {
    let mut rig = Rig::new((640.0, 480.0));
    rig.open(1, &context(64));
    let items = rig.menu_state().items().to_vec();
    for _ in 0..items.len() {
        rig.key(NamedKey::ArrowDown);
        let state = rig.menu_state();
        let Some(i) = state.highlighted() else {
            unreachable!("highlight exists");
        };
        assert!(
            items[i].enabled && items[i].kind == MenuItemKind::Action,
            "item {i}"
        );
        assert!(rig.highlight_in_view(), "item {i}");
    }
}

#[test]
fn the_wheel_scrolls_an_open_menu_one_row_per_notch_and_reaches_the_end() {
    for scale in [1.0, 2.0] {
        let mut rig = Rig::new((640.0, 480.0));
        rig.open(3, &context(64));
        let body = rig.body();
        let view = rig.bounds(body);
        let at = ((view.x + 20) as f32, (view.y + 20) as f32);
        let row = widgets::row_height(&rig.scales);
        assert!(rig.wheel(at, 1.0, scale));
        assert_eq!(rig.ws.tree.scroll_y(body), Some(row));
        rig.relayout();
        for _ in 0..400 {
            rig.wheel(at, 5.0, scale);
        }
        let max = rig.ws.tree.scroll_range(body).unwrap_or(0.0);
        assert_eq!(rig.ws.tree.scroll_y(body), Some(max));
        // Over the canvas (outside the popup) the wheel is not the menu's.
        assert!(!rig.wheel((620.0, 470.0), 1.0, scale));
    }
}

#[test]
fn a_click_after_the_wheel_runs_the_command_under_the_pointer() {
    let mut rig = Rig::new((640.0, 480.0));
    rig.open(3, &context(64));
    let body = rig.body();
    let view = rig.bounds(body);
    let at = ((view.x + 20) as f32, (view.y + 20) as f32);
    for _ in 0..8 {
        rig.wheel(at, 3.0, 1.0);
    }
    rig.relayout();
    // Pick the first enabled action wholly inside the viewport, not row 0.
    let ids = rig.menu_state().item_ids().to_vec();
    let items = rig.menu_state().items().to_vec();
    let commands = rig
        .state
        .open
        .clone()
        .map(|o| o.commands)
        .unwrap_or_default();
    let view = rig.bounds(body);
    let Some(target) = (0..ids.len()).find(|&i| {
        let b = rig.bounds(ids[i]);
        items[i].enabled
            && items[i].kind == MenuItemKind::Action
            && b.y >= view.y
            && b.bottom() <= view.bottom()
    }) else {
        unreachable!("a row is visible");
    };
    assert!(target > 5, "scrolled past the top: {target}");
    let b = rig.bounds(ids[target]);
    let action = rig.click_at(((b.x + 10) as f32, (b.y + i64::from(b.height / 2)) as f32));
    let Some(Some(command)) = commands.get(target).cloned() else {
        unreachable!("an action has a command");
    };
    assert_eq!(action, BarAction::Run(command));
}

#[test]
fn a_press_on_the_menus_scrollbar_scrolls_it_and_does_not_dismiss() {
    let mut rig = Rig::new((640.0, 480.0));
    rig.open(3, &context(64));
    let bar = rig.menu_state().scrollbar().unwrap_or(rig.popup());
    let b = rig.bounds(bar);
    let low = (
        (b.x + i64::from(b.width / 2)) as f32,
        (b.bottom() - 4) as f32,
    );
    assert!(!rig.state.light_dismiss(&mut rig.ws, &mut rig.focus, low));
    rig.click_at(low);
    assert!(rig.state.open.is_some(), "the menu is still open");
    assert!(rig.ws.tree.scroll_y(rig.body()).unwrap_or(0.0) > 0.0);
    // A press on the canvas still closes it.
    assert!(
        rig.state
            .light_dismiss(&mut rig.ws, &mut rig.focus, (630.0, 470.0))
    );
}
