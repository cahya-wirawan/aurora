//! The in-window menu bar (0.225.0): a `Role::MenuBar` row of flat text
//! buttons, one per menu, for Linux and Windows (macOS keeps its native
//! menu and hides this bar unless forced on).
//!
//! The bar is the first child of the canvas column, above the options bar,
//! so it spans the canvas column and not the tools panel or the rail (a
//! full-window row would need the root re-parented; a named gap). It is a
//! `WidgetKind::Panel` (`surface.panel` with the panel's `border.default`
//! outline) whose accessibility role is overridden to `MenuBar`, and is
//! built **hidden**: [`set_menu_bar_shown`] shows it, so every layout that
//! does not ask for it is exactly as before.
//!
//! Each title is a [`widgets::insert_flat_button`]: a `Role::Button` named
//! after the menu that reports `expanded` while its popup is open and is
//! its own `Tab` stop, like a tools-panel button. The popup is the
//! toolkit's [`widgets::open_menu`] (a `Role::Menu` popover with
//! `Role::MenuItem` children), inserted under the bar right below the
//! button by [`open_menu_under`]. What the items *are* and what they do is
//! `aurora-app`'s: this module only builds, shows, relabels and measures.
//!
//! Colours and sizes are existing tokens only: `surface.panel`,
//! `border.default`, `text.primary`, `state.hover_overlay`,
//! `state.active_overlay`, `spacing.*` and the control row height.

use accesskit::{Node, Orientation, Role};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, MenuItem, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::{FlexDirection, Style};

/// The bar's accessible label.
pub const MENU_BAR_LABEL: &str = "Menu bar";

/// The menus' titles, left to right. `aurora-app`'s menu model lists the
/// same six (a test pins them together).
pub const MENU_BAR_TITLES: [&str; 6] = ["File", "Edit", "Layer", "Select", "View", "Window"];

/// The bar: its root row and one button per title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuBar {
    pub root: WidgetId,
    pub buttons: [WidgetId; 6],
}

impl MenuBar {
    /// Which menu `id` is the title button of.
    #[must_use]
    pub fn index_of(&self, id: WidgetId) -> Option<usize> {
        self.buttons.iter().position(|button| *button == id)
    }
}

#[allow(clippy::cast_precision_loss)]
fn bar_style(scales: &Scales) -> Style {
    let (xxs, sm) = (scales.spacing.xxs as f32, scales.spacing.sm as f32);
    Style {
        flex_direction: FlexDirection::Row,
        flex_shrink: 0.0,
        align_items: Some(taffy::AlignItems::CENTER),
        gap: taffy::Size {
            width: taffy::style_helpers::length(xxs),
            height: taffy::style_helpers::length(xxs),
        },
        padding: taffy::Rect {
            left: taffy::style_helpers::length(xxs),
            right: taffy::style_helpers::length(xxs),
            top: taffy::style_helpers::zero(),
            bottom: taffy::style_helpers::zero(),
        },
        min_size: taffy::Size {
            width: taffy::style_helpers::zero(),
            height: taffy::style_helpers::length(widgets::row_height(scales) + 2.0 * sm),
        },
        ..Default::default()
    }
}

/// Builds the bar under `parent`, hidden.
pub(crate) fn insert_menu_bar(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
) -> Result<MenuBar, WidgetError> {
    let mut node = Node::new(Role::MenuBar);
    node.set_label(MENU_BAR_LABEL);
    node.set_orientation(Orientation::Horizontal);
    let root = tree.insert(parent, bar_style(scales), node, WidgetKind::Panel)?;
    let mut buttons = [root; 6];
    for (slot, title) in buttons.iter_mut().zip(MENU_BAR_TITLES) {
        match widgets::insert_flat_button(tree, root, scales, title) {
            Ok(id) => *slot = id,
            Err(err) => {
                let _ = tree.remove(root);
                return Err(err);
            }
        }
    }
    crate::workspace::set_shown(tree, root, false)?;
    Ok(MenuBar { root, buttons })
}

/// Shows or hides the bar: its `display` and its AT `hidden` state.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed workspace.
pub fn set_menu_bar_shown(
    tree: &mut WidgetTree<WidgetKind>,
    bar: &MenuBar,
    shown: bool,
) -> Result<(), WidgetError> {
    crate::workspace::set_shown(tree, bar.root, shown)
}

/// Whether the bar is shown.
#[must_use]
pub fn menu_bar_shown(tree: &WidgetTree<WidgetKind>, bar: &MenuBar) -> bool {
    tree.style(bar.root)
        .is_some_and(|style| style.display != taffy::Display::None)
}

/// Whether `id` is the bar, one of its buttons, or anything inside an open
/// popup (a popup is a child of the bar's root): the tree-only form
/// `aurora-app`'s widget-owner lookup uses.
#[must_use]
pub fn is_menu_bar_part(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    let mut current = Some(id);
    while let Some(node) = current {
        if tree
            .accessibility(node)
            .is_some_and(|n| n.role() == Role::MenuBar)
        {
            return true;
        }
        current = tree.parent(node);
    }
    false
}

/// Marks the title of the open menu (`open`) and clears the others: each
/// button's toggle state, which its node reports as `expanded`. Returns
/// whether anything changed.
///
/// # Errors
///
/// A [`WidgetError`] for a button that is not in the tree.
pub fn sync_menu_bar_open(
    tree: &mut WidgetTree<WidgetKind>,
    bar: &MenuBar,
    open: Option<usize>,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    for (index, button) in bar.buttons.iter().enumerate() {
        changed |= widgets::set_button_toggled(tree, *button, open == Some(index))?;
    }
    Ok(changed)
}

/// Tints the title under the pointer (`None`: none). Returns whether
/// anything changed.
///
/// # Errors
///
/// A [`WidgetError`] for a button that is not in the tree.
pub fn set_menu_bar_hover(
    tree: &mut WidgetTree<WidgetKind>,
    bar: &MenuBar,
    hovered: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    for button in &bar.buttons {
        changed |= widgets::set_button_hovered(tree, *button, hovered == Some(*button))?;
    }
    Ok(changed)
}

/// The width of a popup whose widest item text is `widest_text` logical px
/// wide: the text plus `spacing.sm` of room on each side of it and the
/// same again for the row's own inset. Never below the control row height
/// times four, so a short menu is not a sliver.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn menu_popup_width(scales: &Scales, widest_text: f32) -> f32 {
    let room = 4.0 * scales.spacing.sm as f32;
    let floor = 4.0 * widgets::row_height(scales);
    (widest_text.max(0.0) + room).max(floor)
}

/// Opens menu `index`'s popup under its title button: the toolkit's menu,
/// a child of the bar's root at the button's left edge and the bar's
/// bottom edge. The caller focuses the returned id.
///
/// # Errors
///
/// A [`WidgetError`] when `index` is out of range, the bar has no layout
/// yet (no bounds) or the toolkit refuses the menu (no action, bad width).
pub fn open_menu_under(
    tree: &mut WidgetTree<WidgetKind>,
    bar: &MenuBar,
    scales: &Scales,
    index: usize,
    width: f32,
    items: Vec<MenuItem>,
) -> Result<WidgetId, WidgetError> {
    let button = *bar.buttons.get(index).ok_or(WidgetError::IndexOutOfRange {
        index,
        len: bar.buttons.len(),
    })?;
    let (Some(button_bounds), Some(root_bounds)) = (tree.bounds(button), tree.bounds(bar.root))
    else {
        return Err(WidgetError::UnknownWidget(button));
    };
    #[allow(clippy::cast_precision_loss)]
    let at = (
        (button_bounds.x - root_bounds.x) as f32,
        root_bounds.height as f32,
    );
    let title = MENU_BAR_TITLES.get(index).copied().unwrap_or_default();
    widgets::open_menu(tree, bar.root, scales, title, at, width, items)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)] // fixed-size fixtures built a few lines up
mod tests {
    use super::*;
    use crate::workspace::build_workspace;

    fn scales() -> Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn laid_out(shown: bool) -> crate::Workspace {
        let mut ws = build_workspace(&scales());
        if let Err(err) = set_menu_bar_shown(&mut ws.tree, &ws.menu_bar.clone(), shown) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1280.0, 800.0);
        ws
    }

    #[test]
    fn the_bar_is_built_hidden_and_does_not_move_the_canvas() {
        let ws = laid_out(false);
        assert!(!menu_bar_shown(&ws.tree, &ws.menu_bar));
        let Some(node) = ws.tree.accessibility(ws.menu_bar.root) else {
            unreachable!("built");
        };
        assert!(node.is_hidden());
        assert_eq!(node.role(), Role::MenuBar);
        let hidden_top = ws.tree.bounds(ws.options_bar).map(|b| b.y);
        let shown = laid_out(true);
        let shown_top = shown.tree.bounds(shown.options_bar).map(|b| b.y);
        let Some(bar) = shown.tree.bounds(shown.menu_bar.root) else {
            unreachable!("shown bar has bounds");
        };
        assert!(bar.height > 0);
        assert_eq!(
            shown_top.zip(hidden_top).map(|(s, h)| s - h),
            Some(i64::from(bar.height)),
            "showing the bar pushes the options bar down by exactly its height"
        );
    }

    #[test]
    fn the_bar_is_a_menubar_of_six_named_buttons_in_order() {
        let ws = laid_out(true);
        let Some(node) = ws.tree.accessibility(ws.menu_bar.root) else {
            unreachable!("built");
        };
        assert_eq!(node.role(), Role::MenuBar);
        assert_eq!(node.label(), Some(MENU_BAR_LABEL));
        assert!(!node.is_hidden());
        assert_eq!(
            ws.tree.children(ws.menu_bar.root),
            Some(ws.menu_bar.buttons.as_slice())
        );
        for (button, title) in ws.menu_bar.buttons.iter().zip(MENU_BAR_TITLES) {
            let Some(node) = ws.tree.accessibility(*button) else {
                unreachable!("built");
            };
            assert_eq!(node.role(), Role::Button);
            assert_eq!(node.label(), Some(title));
            assert_eq!(node.is_expanded(), Some(false), "{title}");
            assert_eq!(node.toggled(), None, "{title}: not a toggle for AT");
            assert!(node.supports_action(accesskit::Action::Focus));
            assert!(node.supports_action(accesskit::Action::Click));
            assert!(is_menu_bar_part(&ws.tree, *button));
        }
        assert!(!is_menu_bar_part(&ws.tree, ws.options_bar));
        assert_eq!(ws.menu_bar.index_of(ws.menu_bar.buttons[3]), Some(3));
        assert_eq!(ws.menu_bar.index_of(ws.canvas_area), None);
    }

    #[test]
    fn syncing_the_open_menu_reports_expanded_on_that_title_only() {
        let mut ws = laid_out(true);
        let bar = ws.menu_bar;
        assert_eq!(
            sync_menu_bar_open(&mut ws.tree, &bar, Some(2)).ok(),
            Some(true)
        );
        assert_eq!(
            sync_menu_bar_open(&mut ws.tree, &bar, Some(2)).ok(),
            Some(false)
        );
        for (index, button) in bar.buttons.iter().enumerate() {
            let expanded = ws
                .tree
                .accessibility(*button)
                .and_then(accesskit::Node::is_expanded);
            assert_eq!(expanded, Some(index == 2), "button {index}");
        }
        assert_eq!(
            sync_menu_bar_open(&mut ws.tree, &bar, None).ok(),
            Some(true)
        );
        let any = bar.buttons.iter().any(|b| {
            ws.tree
                .accessibility(*b)
                .and_then(accesskit::Node::is_expanded)
                == Some(true)
        });
        assert!(!any);
    }

    #[test]
    fn a_popup_opens_under_its_button_as_a_menu_of_menu_items() {
        let mut ws = laid_out(true);
        let bar = ws.menu_bar;
        let items = vec![
            MenuItem::action("One"),
            MenuItem {
                enabled: false,
                ..MenuItem::action("Two")
            },
            MenuItem::separator(),
            MenuItem::action("Three"),
        ];
        let menu = match open_menu_under(&mut ws.tree, &bar, &scales(), 1, 200.0, items) {
            Ok(menu) => menu,
            Err(err) => unreachable!("{err:?}"),
        };
        ws.tree.compute_layout(1280.0, 800.0);
        assert_eq!(ws.tree.parent(menu), Some(bar.root));
        assert!(is_menu_bar_part(&ws.tree, menu));
        let (Some(m), Some(b), Some(r)) = (
            ws.tree.bounds(menu),
            ws.tree.bounds(bar.buttons[1]),
            ws.tree.bounds(bar.root),
        ) else {
            unreachable!("laid out");
        };
        assert_eq!(m.x, b.x, "left edges align");
        assert_eq!(m.y, r.y + i64::from(r.height), "directly under the bar");
        assert_eq!(m.width, 200);
        let Some(node) = ws.tree.accessibility(menu) else {
            unreachable!("built");
        };
        assert_eq!(node.role(), Role::Menu);
        assert_eq!(node.label(), Some("Edit"));
        let state = match widgets::menu_state(&ws.tree, menu) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        };
        let roles: Vec<Role> = state
            .item_ids()
            .iter()
            .filter_map(|id| ws.tree.accessibility(*id).map(Node::role))
            .collect();
        assert_eq!(
            roles,
            [
                Role::MenuItem,
                Role::MenuItem,
                Role::Splitter,
                Role::MenuItem
            ]
        );
        let two = ws.tree.accessibility(state.item_ids()[1]);
        assert!(
            two.is_some_and(Node::is_disabled),
            "a disabled item is disabled"
        );
    }

    #[test]
    fn opening_with_a_bad_index_or_no_action_is_an_error() {
        let mut ws = build_workspace(&scales());
        let bar = ws.menu_bar;
        let items = vec![MenuItem::action("x")];
        ws.tree.compute_layout(800.0, 600.0);
        let only_separator = vec![MenuItem::separator()];
        assert!(open_menu_under(&mut ws.tree, &bar, &scales(), 0, 100.0, only_separator).is_err());
        assert!(open_menu_under(&mut ws.tree, &bar, &scales(), 9, 100.0, items).is_err());
    }

    #[test]
    fn popup_width_has_room_around_the_text_and_a_floor() {
        let s = scales();
        assert!(menu_popup_width(&s, 300.0) > 300.0);
        assert!(menu_popup_width(&s, 0.0) > 0.0);
        assert!(menu_popup_width(&s, 10.0) >= 4.0 * widgets::row_height(&s));
        assert!(menu_popup_width(&s, f32::NAN) > 0.0);
    }
}
