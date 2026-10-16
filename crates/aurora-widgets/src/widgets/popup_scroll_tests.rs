//! Menus and dropdown lists that stay on screen and scroll (0.259.0): a
//! popup is capped to the room the window has, its rows keep a fixed
//! pitch, and the keyboard, the wheel and a click all reach every row.

#![allow(clippy::indexing_slicing, clippy::cast_precision_loss)]

use super::{
    DropdownKey, MenuItem, MenuKey, MenuOutcome, WidgetKind, activate_menu_item,
    commit_dropdown_row, dropdown_state, handle_dropdown_key, handle_menu_key, insert_dropdown,
    menu_state, new_tree, open_menu, popup_scroll_target, row_height, test_scales,
};
use crate::tree::{WidgetId, WidgetTree};
use crate::{WidgetError, compute_text_layout};
use taffy::style_helpers::length;
use taffy::{Size, Style};

fn ok<T>(result: Result<T, WidgetError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn window(width: f32, height: f32) -> (WidgetTree<WidgetKind>, WidgetId) {
    let (mut tree, root) = new_tree(Style {
        size: Size {
            width: length(width),
            height: length(height),
        },
        ..Default::default()
    });
    compute_text_layout(&mut tree, width, height, None);
    (tree, root)
}

fn lay(tree: &mut WidgetTree<WidgetKind>, size: (f32, f32)) {
    compute_text_layout(tree, size.0, size.1, None);
}

/// Sizes the tests run at: the app's default window, a small one, the
/// minimum window, and the minimum window's logical size at scale 2
/// (a 1274x900 physical-ish window halves; 637x450).
const SIZES: [(f32, f32); 5] = [
    (1274.0, 900.0),
    (800.0, 600.0),
    (640.0, 480.0),
    (637.0, 450.0),
    (400.0, 240.0),
];

/// `n` rows: actions, with a separator after every eighth and every
/// seventh action disabled.
fn many_items(n: usize) -> Vec<MenuItem> {
    let mut items = Vec::new();
    for index in 0..n {
        let mut item = MenuItem::action(&format!("Item {index}"));
        item.enabled = index % 7 != 3;
        items.push(item);
        if index % 8 == 7 {
            items.push(MenuItem::separator());
        }
    }
    items
}

struct Open {
    tree: WidgetTree<WidgetKind>,
    menu: WidgetId,
    size: (f32, f32),
}

fn open(size: (f32, f32), at: (f32, f32), items: Vec<MenuItem>) -> Open {
    let (mut tree, root) = window(size.0, size.1);
    let menu = ok(open_menu(
        &mut tree,
        root,
        &test_scales(),
        "Select",
        at,
        200.0,
        items,
    ));
    lay(&mut tree, size);
    Open { tree, menu, size }
}

impl Open {
    fn body(&self) -> WidgetId {
        match ok(menu_state(&self.tree, self.menu)).body() {
            Some(body) => body,
            None => unreachable!("an open menu has a body"),
        }
    }

    fn ids(&self) -> Vec<WidgetId> {
        ok(menu_state(&self.tree, self.menu)).item_ids().to_vec()
    }

    fn bounds(&self, id: WidgetId) -> aurora_core::Rect {
        match self.tree.bounds(id) {
            Some(bounds) => bounds,
            None => unreachable!("laid out"),
        }
    }

    fn press(&mut self, key: MenuKey) -> MenuOutcome {
        let outcome = ok(handle_menu_key(&mut self.tree, self.menu, key));
        lay(&mut self.tree, self.size);
        outcome
    }

    fn highlighted_inside_viewport(&self) -> bool {
        let state = ok(menu_state(&self.tree, self.menu));
        let (Some(row), view) = (
            state.highlighted().and_then(|i| state.item_ids().get(i)),
            self.bounds(self.body()),
        ) else {
            return false;
        };
        let row = self.bounds(*row);
        row.y >= view.y && row.bottom() <= view.bottom()
    }
}

#[test]
fn a_menu_never_extends_past_the_window_at_any_size() {
    for size in SIZES {
        for count in [5, 25, 94] {
            let menu = open(size, (40.0, 30.0), many_items(count));
            let frame = menu.bounds(menu.menu);
            assert!(
                frame.bottom() <= size.1 as i64,
                "{count} items in {size:?}: {frame:?}"
            );
            assert!(frame.y >= 30, "it opens at its place");
        }
    }
}

#[test]
fn a_short_menu_is_not_capped_and_shows_no_scrollbar() {
    let menu = open((1274.0, 900.0), (40.0, 30.0), many_items(5));
    let ids = menu.ids();
    let last = menu.bounds(ids[ids.len() - 1]);
    let first = menu.bounds(ids[0]);
    assert_eq!(
        i64::from(last.height) + last.y - first.y,
        i64::from(menu.bounds(menu.body()).height),
        "the body is exactly its rows"
    );
    let bar = ok(menu_state(&menu.tree, menu.menu)).scrollbar();
    assert_eq!(
        bar.and_then(|b| menu.tree.style(b)).map(|s| s.display),
        Some(taffy::Display::None)
    );
    assert_eq!(menu.tree.scroll_range(menu.body()), Some(0.0));
}

#[test]
fn a_tall_menu_is_capped_to_the_window_less_the_margin_and_scrolls() {
    let scales = test_scales();
    let menu = open((640.0, 480.0), (40.0, 30.0), many_items(94));
    let frame = menu.bounds(menu.menu);
    let margin = scales.spacing.sm as i64;
    assert_eq!(frame.bottom(), 480 - margin, "capped to window - margin");
    assert!(menu.tree.scroll_range(menu.body()).unwrap_or(0.0) > 0.0);
    let bar = ok(menu_state(&menu.tree, menu.menu)).scrollbar();
    assert_eq!(
        bar.and_then(|b| menu.tree.style(b)).map(|s| s.display),
        Some(taffy::Display::Flex),
        "the scrollbar shows"
    );
    let bar_bounds = menu.bounds(bar.unwrap_or(menu.menu));
    assert!(
        bar_bounds.height > 0 && bar_bounds.width > 0,
        "{bar_bounds:?}"
    );
}

#[test]
fn rows_keep_a_fixed_pitch_and_never_overlap_before_and_after_scrolling() {
    let scales = test_scales();
    let row = row_height(&scales) as i64;
    let sep = scales.spacing.xxs as i64;
    let mut menu = open((640.0, 480.0), (40.0, 30.0), many_items(94));
    let items = ok(menu_state(&menu.tree, menu.menu)).items().to_vec();
    for offset in [0.0, 5.0 * row as f32, 40.0 * row as f32] {
        ok(menu.tree.set_scroll_y(menu.body(), offset));
        let ids = menu.ids();
        for pair in 0..ids.len() - 1 {
            let (a, b) = (menu.bounds(ids[pair]), menu.bounds(ids[pair + 1]));
            let height = match items[pair].kind {
                super::MenuItemKind::Action => row,
                super::MenuItemKind::Separator => sep,
            };
            assert_eq!(i64::from(a.height), height, "item {pair}");
            assert_eq!(b.y - a.y, height, "item {pair} to the next at {offset}");
        }
    }
}

#[test]
fn end_reaches_the_last_item_and_keeps_it_in_view_then_home_returns() {
    for size in SIZES {
        let mut menu = open(size, (40.0, 30.0), many_items(94));
        let last_enabled = {
            let state = ok(menu_state(&menu.tree, menu.menu));
            state
                .items()
                .iter()
                .rposition(|item| item.enabled && item.kind == super::MenuItemKind::Action)
        };
        assert!(
            matches!(menu.press(MenuKey::End), MenuOutcome::Moved(i) if Some(i) == last_enabled)
        );
        assert!(menu.highlighted_inside_viewport(), "{size:?}");
        let max = menu.tree.scroll_range(menu.body()).unwrap_or(0.0);
        assert_eq!(menu.tree.scroll_y(menu.body()), Some(max), "{size:?}");
        assert!(matches!(menu.press(MenuKey::Home), MenuOutcome::Moved(0)));
        assert_eq!(menu.tree.scroll_y(menu.body()), Some(0.0));
        assert!(menu.highlighted_inside_viewport());
    }
}

#[test]
fn up_from_the_first_item_wraps_to_the_last_and_scrolls_to_it() {
    let mut menu = open((640.0, 480.0), (40.0, 30.0), many_items(94));
    let outcome = menu.press(MenuKey::Up);
    assert!(
        matches!(outcome, MenuOutcome::Moved(i) if i > 90),
        "{outcome:?}"
    );
    assert!(menu.highlighted_inside_viewport());
    assert!(menu.tree.scroll_y(menu.body()).unwrap_or(0.0) > 0.0);
    // And Down wraps back to the top.
    assert!(matches!(menu.press(MenuKey::Down), MenuOutcome::Moved(0)));
    assert_eq!(menu.tree.scroll_y(menu.body()), Some(0.0));
}

#[test]
fn down_visits_every_enabled_item_in_order_keeping_each_in_view() {
    let mut menu = open((640.0, 480.0), (40.0, 30.0), many_items(94));
    let enabled: Vec<usize> = ok(menu_state(&menu.tree, menu.menu))
        .items()
        .iter()
        .enumerate()
        .filter(|(_, item)| item.enabled && item.kind == super::MenuItemKind::Action)
        .map(|(i, _)| i)
        .collect();
    for &expected in &enabled[1..] {
        assert_eq!(menu.press(MenuKey::Down), MenuOutcome::Moved(expected));
        assert!(menu.highlighted_inside_viewport(), "item {expected}");
    }
}

#[test]
fn page_down_and_page_up_move_by_a_viewport_without_wrapping() {
    let mut menu = open((640.0, 480.0), (40.0, 30.0), many_items(94));
    let first = ok(menu_state(&menu.tree, menu.menu)).highlighted();
    let moved = menu.press(MenuKey::PageDown);
    let MenuOutcome::Moved(target) = moved else {
        unreachable!("{moved:?}");
    };
    let rows_shown = i64::from(menu.bounds(menu.body()).height) / row_height(&test_scales()) as i64;
    assert!(
        target as i64 >= rows_shown - 2 && (target as i64) <= rows_shown + 3,
        "about a page: {target} for {rows_shown} rows"
    );
    assert!(menu.highlighted_inside_viewport());
    assert!(matches!(menu.press(MenuKey::PageUp), MenuOutcome::Moved(i) if i < target));
    assert!(menu.highlighted_inside_viewport());
    let mut guard = 0;
    while !matches!(menu.press(MenuKey::PageUp), MenuOutcome::Ignored) {
        guard += 1;
        assert!(guard < 5, "page up reaches the top");
    }
    assert_eq!(ok(menu_state(&menu.tree, menu.menu)).highlighted(), first);
    assert_eq!(menu.tree.scroll_y(menu.body()), Some(0.0));
    let mut guard = 0;
    while !matches!(menu.press(MenuKey::PageDown), MenuOutcome::Ignored) {
        guard += 1;
        assert!(guard < 40, "page down reaches the end");
    }
    assert!(menu.highlighted_inside_viewport());
}

#[test]
fn the_wheel_target_is_the_open_list_and_nothing_elsewhere() {
    let mut menu = open((640.0, 480.0), (40.0, 30.0), many_items(94));
    let ids = menu.ids();
    let row = menu.bounds(ids[2]);
    let over_row = ((row.x + 5) as f32, (row.y + 5) as f32);
    assert_eq!(popup_scroll_target(&menu.tree, over_row), Some(menu.body()));
    let bar = ok(menu_state(&menu.tree, menu.menu))
        .scrollbar()
        .unwrap_or(menu.menu);
    let b = menu.bounds(bar);
    assert_eq!(
        popup_scroll_target(&menu.tree, ((b.x + 2) as f32, (b.y + 10) as f32)),
        Some(menu.body()),
        "the scrollbar is part of the list"
    );
    assert_eq!(popup_scroll_target(&menu.tree, (620.0, 20.0)), None);
    // Scrolling by the wheel's route reaches the end.
    ok(menu.tree.scroll_by(menu.body(), 1.0e6));
    let max = menu.tree.scroll_range(menu.body()).unwrap_or(0.0);
    assert_eq!(menu.tree.scroll_y(menu.body()), Some(max));
}

#[test]
fn a_click_on_an_item_after_scrolling_runs_the_item_under_the_pointer() {
    let mut menu = open((640.0, 480.0), (40.0, 30.0), many_items(94));
    let ids = menu.ids();
    ok(menu.tree.set_scroll_y(menu.body(), 1.0e6));
    // The last item is now at the bottom of the viewport.
    let view = menu.bounds(menu.body());
    let probe = ((view.x + 20) as f32, (view.bottom() - 3) as f32);
    let Some(hit) = menu.tree.hit_test(probe) else {
        unreachable!("the viewport shows rows");
    };
    let Some(index) = ids.iter().position(|&id| id == hit) else {
        unreachable!("an item is hit, got {hit:?}");
    };
    assert!(index > 85, "it is a late item: {index}");
    let outcome = ok(activate_menu_item(&mut menu.tree, menu.menu, hit));
    assert_eq!(outcome, MenuOutcome::Activated(index));
    assert!(!menu.tree.contains(menu.menu), "the menu closed");
}

#[test]
fn rows_outside_the_viewport_stay_in_the_accessibility_tree() {
    let menu = open((640.0, 480.0), (40.0, 30.0), many_items(94));
    let ids = menu.ids();
    let view = menu.bounds(menu.body());
    let outside = ids
        .iter()
        .filter(|&&id| menu.bounds(id).y >= view.bottom())
        .count();
    assert!(outside > 40, "most rows are scrolled out: {outside}");
    let update = menu.tree.accessibility_update(menu.tree.root());
    let known: std::collections::HashSet<_> = update.nodes.iter().map(|(id, _)| *id).collect();
    for &id in &ids {
        assert!(menu.tree.accessibility(id).is_some());
    }
    assert!(known.len() >= ids.len(), "every row is in the update");
}

#[test]
fn an_unlaid_out_tree_gets_no_cap() {
    let (mut tree, root) = new_tree(Style::default());
    let menu = ok(open_menu(
        &mut tree,
        root,
        &test_scales(),
        "Edit",
        (0.0, 0.0),
        100.0,
        many_items(30),
    ));
    let style = tree.style(menu).cloned().unwrap_or_default();
    assert_eq!(style.max_size.height, taffy::style_helpers::auto());
}

// ---- Dropdown lists ----------------------------------------------------

fn modes() -> Vec<String> {
    (0..28).map(|i| format!("Mode {i}")).collect()
}

struct Combo {
    tree: WidgetTree<WidgetKind>,
    id: WidgetId,
    size: (f32, f32),
}

fn combo(size: (f32, f32), selected: Option<usize>, offset_y: f32) -> Combo {
    let (mut tree, root) = window(size.0, size.1);
    let column = ok(tree.insert(
        root,
        Style {
            flex_direction: taffy::FlexDirection::Column,
            size: Size {
                width: length(220.0_f32),
                height: taffy::style_helpers::auto(),
            },
            padding: taffy::Rect {
                left: length(10.0_f32),
                right: length(0.0_f32),
                top: length(offset_y),
                bottom: length(0.0_f32),
            },
            ..Default::default()
        },
        accesskit::Node::new(accesskit::Role::GenericContainer),
        WidgetKind::Container,
    ));
    let id = ok(insert_dropdown(
        &mut tree,
        column,
        &test_scales(),
        "Blend mode",
        modes(),
        selected,
    ));
    lay(&mut tree, size);
    Combo { tree, id, size }
}

impl Combo {
    fn open(&mut self) {
        let outcome = ok(handle_dropdown_key(
            &mut self.tree,
            self.id,
            DropdownKey::Down,
        ));
        assert_eq!(outcome, super::DropdownOutcome::Opened);
        lay(&mut self.tree, self.size);
    }

    fn state(&self) -> &super::DropdownState {
        ok(dropdown_state(&self.tree, self.id))
    }

    fn bounds(&self, id: WidgetId) -> aurora_core::Rect {
        match self.tree.bounds(id) {
            Some(bounds) => bounds,
            None => unreachable!("laid out"),
        }
    }

    fn list(&self) -> WidgetId {
        match self.state().list() {
            Some(list) => list,
            None => unreachable!("open"),
        }
    }

    fn body(&self) -> WidgetId {
        match self.state().body() {
            Some(body) => body,
            None => unreachable!("open"),
        }
    }

    fn highlighted_inside_viewport(&self) -> bool {
        let state = self.state();
        let Some(row) = state.highlighted_row() else {
            return false;
        };
        let (row, view) = (self.bounds(row), self.bounds(self.body()));
        row.y >= view.y && row.bottom() <= view.bottom()
    }
}

#[test]
fn a_dropdown_list_with_28_options_stays_inside_a_640x480_window() {
    for y in [20.0_f32, 200.0, 400.0, 455.0] {
        let mut combo = combo((640.0, 480.0), Some(0), y);
        combo.open();
        let list = combo.bounds(combo.list());
        assert!(
            list.y >= 0 && list.bottom() <= 480,
            "control at {y}: list {list:?}"
        );
        assert!(list.height > 0);
    }
}

#[test]
fn a_dropdown_near_the_window_bottom_opens_upward() {
    let mut low = combo((640.0, 480.0), Some(0), 440.0);
    let control = low.bounds(low.id);
    low.open();
    let list = low.bounds(low.list());
    assert_eq!(
        list.bottom(),
        control.y,
        "its bottom edge sits on the control's top"
    );
    let mut high = combo((640.0, 480.0), Some(0), 20.0);
    let control = high.bounds(high.id);
    high.open();
    let list = high.bounds(high.list());
    assert_eq!(
        list.y,
        control.bottom(),
        "below the control when there is room"
    );
}

#[test]
fn a_dropdown_that_fits_is_not_capped() {
    let mut combo = combo((1274.0, 900.0), Some(0), 20.0);
    combo.open();
    let list = combo.bounds(combo.list());
    assert_eq!(
        i64::from(list.height),
        28 * row_height(&test_scales()) as i64,
        "28 rows of one row each"
    );
    assert_eq!(combo.tree.scroll_range(combo.body()), Some(0.0));
}

#[test]
fn dropdown_keys_keep_the_highlight_in_view_and_end_home_work() {
    let mut combo = combo((640.0, 300.0), Some(0), 20.0);
    combo.open();
    assert!(combo.tree.scroll_range(combo.body()).unwrap_or(0.0) > 0.0);
    for expected in 1..28 {
        let outcome = ok(handle_dropdown_key(
            &mut combo.tree,
            combo.id,
            DropdownKey::Down,
        ));
        lay(&mut combo.tree, combo.size);
        assert_eq!(outcome, super::DropdownOutcome::HighlightMoved(expected));
        assert!(combo.highlighted_inside_viewport(), "option {expected}");
    }
    ok(handle_dropdown_key(
        &mut combo.tree,
        combo.id,
        DropdownKey::Home,
    ));
    lay(&mut combo.tree, combo.size);
    assert_eq!(combo.state().highlighted(), Some(0));
    assert_eq!(combo.tree.scroll_y(combo.body()), Some(0.0));
    ok(handle_dropdown_key(
        &mut combo.tree,
        combo.id,
        DropdownKey::End,
    ));
    lay(&mut combo.tree, combo.size);
    assert_eq!(combo.state().highlighted(), Some(27));
    assert!(combo.highlighted_inside_viewport());
}

#[test]
fn opening_with_a_far_selection_scrolls_it_into_view_after_the_first_layout() {
    let mut combo = combo((640.0, 300.0), Some(25), 20.0);
    combo.open();
    assert!(combo.highlighted_inside_viewport());
    assert!(combo.tree.scroll_y(combo.body()).unwrap_or(0.0) > 0.0);
}

#[test]
fn a_row_clicked_after_scrolling_is_committed() {
    let mut combo = combo((640.0, 300.0), Some(0), 20.0);
    combo.open();
    ok(combo.tree.set_scroll_y(combo.body(), 1.0e6));
    let view = combo.bounds(combo.body());
    let Some(hit) = combo
        .tree
        .hit_test(((view.x + 10) as f32, (view.bottom() - 3) as f32))
    else {
        unreachable!("rows are shown");
    };
    let Some(index) = combo.state().rows().iter().position(|&r| r == hit) else {
        unreachable!(
            "an option row is hit, got {:?} at {view:?}",
            combo.tree.payload(hit)
        );
    };
    assert_eq!(index, 27);
    assert_eq!(super::dropdown_of_row(&combo.tree, hit), Some(combo.id));
    let outcome = ok(commit_dropdown_row(&mut combo.tree, combo.id, hit));
    assert_eq!(
        outcome,
        super::DropdownOutcome::Committed {
            index: 27,
            changed: true
        }
    );
    assert_eq!(combo.state().selected(), Some(27));
}

#[test]
fn the_wheel_target_over_an_open_dropdown_is_its_list() {
    let mut combo = combo((640.0, 300.0), Some(0), 20.0);
    combo.open();
    let view = combo.bounds(combo.body());
    assert_eq!(
        popup_scroll_target(&combo.tree, ((view.x + 10) as f32, (view.y + 10) as f32)),
        Some(combo.body())
    );
}

#[test]
fn a_page_down_moves_one_viewport_less_one_item_exactly() {
    let items: Vec<MenuItem> = (0..94)
        .map(|i| MenuItem::action(&format!("Item {i}")))
        .collect();
    let mut menu = open((640.0, 480.0), (40.0, 30.0), items);
    let rows_shown =
        (i64::from(menu.bounds(menu.body()).height) / row_height(&test_scales()) as i64) as usize;
    assert!(rows_shown > 4 && rows_shown < 94);
    assert_eq!(
        menu.press(MenuKey::PageDown),
        MenuOutcome::Moved(rows_shown - 1)
    );
    assert_eq!(
        menu.press(MenuKey::PageDown),
        MenuOutcome::Moved(2 * (rows_shown - 1))
    );
    assert_eq!(
        menu.press(MenuKey::PageUp),
        MenuOutcome::Moved(rows_shown - 1)
    );
}

#[test]
fn the_parents_offset_counts_toward_the_room_below() {
    // The menu's parent starts 100 px down the window: the popup's
    // absolute top is 100 + 30, so the cap is smaller than for a parent
    // at the top.
    let size = (640.0, 480.0);
    let (mut tree, root) = window(size.0, size.1);
    let parent = ok(tree.insert(
        root,
        Style {
            margin: taffy::Rect {
                left: length(0.0_f32),
                right: length(0.0_f32),
                top: length(100.0_f32),
                bottom: length(0.0_f32),
            },
            size: Size {
                width: length(300.0_f32),
                height: length(40.0_f32),
            },
            ..Default::default()
        },
        accesskit::Node::new(accesskit::Role::GenericContainer),
        WidgetKind::Container,
    ));
    lay(&mut tree, size);
    let menu = ok(open_menu(
        &mut tree,
        parent,
        &test_scales(),
        "Select",
        (0.0, 30.0),
        200.0,
        many_items(94),
    ));
    lay(&mut tree, size);
    let Some(frame) = tree.bounds(menu) else {
        unreachable!("laid out");
    };
    assert_eq!(frame.y, 130);
    let margin = test_scales().spacing.sm as i64;
    assert_eq!(frame.bottom(), 480 - margin);
}
