//! The in-app **Widget Gallery** panel (PLAN.md M1.8, 0.130.0): one live
//! instance of every interactive widget `aurora-widgets` ships, docked at
//! the workspace's right edge, so the toolkit can be clicked and keyed
//! through on real hardware instead of only in headless tests.
//!
//! **Built on open and removed on close — never hidden.** With the gallery
//! closed nothing of it is in the tree at all, so the workspace's layout,
//! hit testing and accessibility tree are exactly what they were before
//! it existed. [`insert_gallery_panel`] builds it as the last child of the
//! workspace root (after the dock rail), [`remove_gallery_panel`] takes
//! the whole subtree back out.
//!
//! **Layout**: two columns inside the panel body. The left one stacks the
//! row-sized controls (button, checkbox, slider, text field, scrollbar,
//! swatch, dropdown, tab bar, the "Open menu" button and a three-row tree
//! view), the right one the two square editors (colour picker, curve
//! editor). Every size is derived from `row_height(scales)` and the
//! spacing tokens — the two engineering multipliers below
//! ([`GALLERY_EDITOR_ROWS`], [`GALLERY_TREE_ROWS`]) are *counts of rows*,
//! not pixel values, the same kind of layout constant `workspace.rs`'s
//! `RAIL_*` widths are. The body clips; below
//! [`gallery_content_height`] the bottom of the left column (tree rows) is
//! clipped and unreachable, since nothing in this toolkit scrolls yet.
//!
//! **What this does not do**, all disclosed rather than hidden: no text
//! is drawn anywhere in this toolkit, so every label is blank on screen
//! and reaches the accessibility tree only; the demo button's
//! [`Tooltip`] is shown by **hover only** (0.131.0: [`gallery_hover`],
//! [`gallery_tick`], [`gallery_next_deadline`] — the owner drives them
//! from pointer moves and its event loop's wake-ups), never by keyboard
//! focus, with no warm-up, and draws its text (0.133.0); a press on the
//! button dismisses it. Drags and text-field typing are routed (see
//! `aurora_widgets`' pointer module doc comment), and since 0.133.0 the
//! field draws its content, a caret while focused, and its selection —
//! but a click does not yet place the caret under the pointer. The panel never shrinks: on a narrow window
//! (640 x 480) it and the dock rail leave the canvas a sliver (18 px),
//! pinned by a test rather than fixed. Nothing here touches a document: every outcome
//! is a widget-state change only.

use std::time::{Duration, Instant};

use accesskit::Orientation;
use aurora_core::ToneCurve;
use aurora_theme::{Color, Scales};
use aurora_widgets::widgets::{self, MenuItem, ScrollbarRange, Tooltip, WidgetKind, row_height};
use aurora_widgets::{
    ActionOutcome, FocusManager, PointerOutcome, WidgetError, WidgetId, WidgetTree,
};
use taffy::style_helpers::{auto, length, zero};
use taffy::{FlexDirection, Rect as LayoutRect, Size, Style};

use crate::panel::{PanelHandle, insert_panel};

/// The square editors' side (colour picker, curve editor), in rows: large
/// enough to aim at a point inside them, small enough that the whole
/// gallery fits beside a usable canvas on a laptop-sized window. An
/// engineering layout multiplier, not a style value — the pixel size is
/// always `GALLERY_EDITOR_ROWS * row_height(scales)`.
pub const GALLERY_EDITOR_ROWS: f32 = 8.0;

/// The tree view's height, in rows: a parent and its two children.
pub const GALLERY_TREE_ROWS: f32 = 3.0;

/// The demo tooltip's show delay. **Not a token**, deliberately — see
/// `aurora_widgets::widgets::tooltip`'s own doc comment ("The show delay
/// is a caller-supplied `Duration`, not a token"). Driven by the owner's
/// event loop through [`gallery_next_deadline`] and [`gallery_tick`].
pub const GALLERY_TOOLTIP_DELAY: Duration = Duration::from_millis(500);

/// The sample colour the demo swatch shows — gallery *content*, the same
/// kind of value as a document's pixels, not a style value a theme
/// should override.
const SAMPLE_SWATCH: Color = Color {
    r: 0x3a,
    g: 0x7b,
    b: 0xd5,
};

/// The colour the demo picker opens on (content, like [`SAMPLE_SWATCH`]).
const SAMPLE_PICKER: Color = Color {
    r: 0xd5,
    g: 0x5a,
    b: 0x3a,
};

/// Every id the gallery owns, so the app can route to it and a test can
/// find each widget.
#[derive(Debug)]
pub struct GalleryPanel {
    pub panel: PanelHandle,
    /// The two-column block inside the panel body.
    pub columns: WidgetId,
    pub button: WidgetId,
    pub checkbox: WidgetId,
    pub slider: WidgetId,
    pub text_field: WidgetId,
    pub scrollbar: WidgetId,
    pub swatch: WidgetId,
    pub dropdown: WidgetId,
    pub tab_bar: WidgetId,
    pub menu_button: WidgetId,
    pub tree_view: WidgetId,
    /// Parent, first child, second child. **The two child handles are
    /// valid only while the parent is expanded**: collapsing a tree row
    /// removes its child rows from the tree (`set_tree_item_expanded`),
    /// so after a collapse `tree_rows[1]`/`[2]` name nothing
    /// (`WidgetTree::contains` is `false`). Re-expanding the parent
    /// rebuilds both children under **new** ids, which
    /// [`apply_gallery_outcome`] writes back here — so re-read this field
    /// after any expand, never cache a child id across one.
    pub tree_rows: [WidgetId; 3],
    pub picker: WidgetId,
    pub curve: WidgetId,
    /// Owned by [`Self::button`]; shown on hover after
    /// [`GALLERY_TOOLTIP_DELAY`] ([`gallery_hover`]/[`gallery_tick`]).
    pub tooltip: Tooltip,
    /// The demo menu, while open (a popover under the panel body, so
    /// [`gallery_contains`] sees it).
    pub open_menu: Option<WidgetId>,
}

#[allow(clippy::cast_precision_loss)]
fn px(token: u32) -> f32 {
    token as f32
}

/// The square editors' side: [`GALLERY_EDITOR_ROWS`] rows.
#[must_use]
pub fn gallery_editor_size(scales: &Scales) -> f32 {
    GALLERY_EDITOR_ROWS * row_height(scales)
}

fn gap(scales: &Scales) -> f32 {
    px(scales.spacing.sm)
}

fn padding(scales: &Scales) -> f32 {
    px(scales.spacing.sm)
}

/// The gallery's fixed width: two editor-wide columns, the gap between
/// them, and the body's padding on both sides.
#[must_use]
pub fn gallery_width(scales: &Scales) -> f32 {
    2.0 * gallery_editor_size(scales) + gap(scales) + 2.0 * padding(scales)
}

/// The height the gallery's content needs, measured from the last layout
/// (the two-column block including its padding, which never shrinks): a
/// panel body shorter than this clips the bottom of the taller column —
/// the left one's tree rows with the default scales — and what is clipped
/// cannot be clicked, since nothing in this toolkit scrolls yet. `None`
/// before the first layout.
#[must_use]
pub fn gallery_content_height(
    tree: &WidgetTree<WidgetKind>,
    gallery: &GalleryPanel,
) -> Option<u32> {
    tree.bounds(gallery.columns).map(|bounds| bounds.height)
}

fn root_style(scales: &Scales) -> Style {
    let width = length(gallery_width(scales));
    Style {
        flex_direction: FlexDirection::Column,
        flex_grow: 0.0,
        flex_shrink: 0.0,
        size: Size {
            width,
            height: auto(),
        },
        min_size: Size {
            width,
            height: zero(),
        },
        ..Default::default()
    }
}

fn columns_style(scales: &Scales) -> Style {
    let pad = length(padding(scales));
    Style {
        flex_direction: FlexDirection::Row,
        flex_shrink: 0.0,
        gap: Size {
            width: length(gap(scales)),
            height: length(gap(scales)),
        },
        padding: LayoutRect {
            left: pad,
            right: pad,
            top: pad,
            bottom: pad,
        },
        ..Default::default()
    }
}

fn column_style(scales: &Scales) -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        flex_shrink: 0.0,
        size: Size {
            width: length(gallery_editor_size(scales)),
            height: auto(),
        },
        gap: Size {
            width: length(gap(scales)),
            height: length(gap(scales)),
        },
        ..Default::default()
    }
}

fn tree_holder_style(scales: &Scales) -> Style {
    Style {
        flex_shrink: 0.0,
        size: Size {
            width: length(gallery_editor_size(scales)),
            height: length(GALLERY_TREE_ROWS * row_height(scales)),
        },
        ..Default::default()
    }
}

/// Builds the gallery as the last child of `parent` (the workspace root)
/// and returns every id it owns. Nothing is left behind on failure.
///
/// # Errors
///
/// Whatever a widget constructor refuses — none can, for these fixed
/// inputs, short of `parent` not existing
/// ([`WidgetError::UnknownWidget`]).
pub fn insert_gallery_panel(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
) -> Result<GalleryPanel, WidgetError> {
    let panel = insert_panel(tree, parent, "Widget Gallery")?;
    let built = tree
        .set_style(panel.root, root_style(scales))
        .and_then(|()| build(tree, panel, scales));
    if built.is_err() {
        let _ = tree.remove(panel.root);
    }
    built
}

fn build(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
) -> Result<GalleryPanel, WidgetError> {
    let columns = widgets::insert_container(tree, panel.body, columns_style(scales))?;
    let left = widgets::insert_container(tree, columns, column_style(scales))?;
    let right = widgets::insert_container(tree, columns, column_style(scales))?;

    let button = widgets::insert_button(tree, left, scales, "Button")?;
    let checkbox = widgets::insert_checkbox(tree, left, scales, "Checkbox")?;
    let slider = widgets::insert_slider(tree, left, scales, "Slider", 50.0, 0.0, 100.0)?;
    let text_field = widgets::insert_text_field(tree, left, scales, "Text field", "")?;
    let scrollbar = widgets::insert_scrollbar(
        tree,
        left,
        scales,
        Orientation::Horizontal,
        Some("Scrollbar"),
        0.0,
        ScrollbarRange {
            min: 0.0,
            max: 100.0,
            page_size: 25.0,
        },
    )?;
    let swatch = widgets::insert_color_swatch(tree, left, scales, SAMPLE_SWATCH)?;
    let dropdown = widgets::insert_dropdown(
        tree,
        left,
        scales,
        "Dropdown",
        vec!["First".into(), "Second".into(), "Third".into()],
        Some(0),
    )?;
    let tab_bar = widgets::insert_tab_bar(
        tree,
        left,
        scales,
        "Tabs",
        vec!["One".into(), "Two".into(), "Three".into()],
        0,
    )?;
    let menu_button = widgets::insert_button(tree, left, scales, "Open menu")?;
    let holder = widgets::insert_container(tree, left, tree_holder_style(scales))?;
    let tree_view = widgets::insert_tree_view(tree, holder, Some("Tree view"))?;
    let parent_row = widgets::insert_tree_item(tree, tree_view, scales, "Parent", true)?;
    let [first_row, second_row] = insert_tree_children(tree, parent_row, scales)?;
    widgets::set_tree_item_expanded(tree, parent_row, true)?;

    let size = gallery_editor_size(scales);
    let picker =
        widgets::insert_color_picker(tree, right, scales, "Colour picker", SAMPLE_PICKER, size)?;
    let curve = widgets::insert_curve_editor(
        tree,
        right,
        scales,
        "Curve editor",
        size,
        ToneCurve::identity(),
    )?;
    let tooltip = Tooltip::new(tree, button, scales, "A tooltip", GALLERY_TOOLTIP_DELAY)?;

    Ok(GalleryPanel {
        panel,
        columns,
        button,
        checkbox,
        slider,
        text_field,
        scrollbar,
        swatch,
        dropdown,
        tab_bar,
        menu_button,
        tree_view,
        tree_rows: [parent_row, first_row, second_row],
        picker,
        curve,
        tooltip,
        open_menu: None,
    })
}

/// The demo tree's two child rows under `parent` — built at insert and
/// rebuilt (under new ids) whenever a collapse removed them and the
/// parent is expanded again.
fn insert_tree_children(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
) -> Result<[WidgetId; 2], WidgetError> {
    let first = widgets::insert_tree_item(tree, parent, scales, "First child", false)?;
    let second = widgets::insert_tree_item(tree, parent, scales, "Second child", false)?;
    Ok([first, second])
}

/// Takes the whole gallery back out of the tree — its open menu and open
/// dropdown list included, since both are descendants of the panel. The
/// caller then runs `FocusManager::validate` (focus may have been inside)
/// and forgets any armed click (`ClickTracker::reset`).
///
/// **The panel root is always removed**, even when detaching the tooltip
/// fails — a failed detach must not orphan the whole panel in the tree
/// with nothing left holding its ids (critic C6).
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if the panel is already gone; otherwise
/// whatever detaching the tooltip refused, reported *after* the panel
/// root is gone.
pub fn remove_gallery_panel(
    tree: &mut WidgetTree<WidgetKind>,
    gallery: GalleryPanel,
) -> Result<(), WidgetError> {
    let mut gallery = gallery;
    // A shown tooltip's node is a popover under the button, so removing
    // the panel would take it too; detaching first also returns the
    // controller to `Idle` so nothing is left pending.
    let detached = gallery.tooltip.detach(tree);
    tree.remove(gallery.panel.root)?;
    detached
}

/// Whether `id` belongs to the gallery — the panel's own subtree, which
/// includes the open menu and the dropdown's open list (both are
/// parented inside it, popovers or not).
#[must_use]
pub fn gallery_contains(
    tree: &WidgetTree<WidgetKind>,
    gallery: &GalleryPanel,
    id: WidgetId,
) -> bool {
    tree.is_within(gallery.panel.root, id)
        || gallery
            .open_menu
            .is_some_and(|menu| tree.is_within(menu, id))
}

/// Light dismiss for a primary `Down` at `point`: closes the gallery's open
/// menu and open dropdown list when the press lands outside them (and,
/// for each, outside the control that opened it — a press there toggles
/// it closed through the ordinary click path instead). Returns whether
/// anything closed — the caller re-runs layout, re-announces and redraws
/// on `true` even when the press itself is not the gallery's. Run it
/// before routing the press itself. Focus that was inside the closed menu
/// goes back to the "Open menu" button (the menu's "the caller owns
/// focus" contract, the same hand-back `Escape` gets), rather than to
/// nothing.
///
/// # Errors
///
/// Whatever `close_menu`/`set_dropdown_open` refuses.
pub fn gallery_light_dismiss(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    gallery: &mut GalleryPanel,
    point: (f32, f32),
) -> Result<bool, WidgetError> {
    forget_stale_menu(tree, gallery);
    let hit = tree.hit_test(point);
    let inside = |tree: &WidgetTree<WidgetKind>, owner: WidgetId| {
        hit.is_some_and(|hit| tree.is_within(owner, hit))
    };
    let mut closed = false;
    if let Some(menu) = gallery.open_menu
        && !inside(tree, menu)
        && !inside(tree, gallery.menu_button)
    {
        let focus_was_inside = focus
            .focused()
            .is_some_and(|focused| tree.is_within(menu, focused));
        widgets::close_menu(tree, menu)?;
        gallery.open_menu = None;
        closed = true;
        if focus_was_inside {
            focus.focus(tree, gallery.menu_button)?;
        }
    }
    if widgets::dropdown_state(tree, gallery.dropdown)?.is_open() && !inside(tree, gallery.dropdown)
    {
        widgets::set_dropdown_open(tree, gallery.dropdown, false)?;
        closed = true;
    }
    focus.validate(tree);
    Ok(closed)
}

/// Reports the pointer's position (`None`: off the window, or owned by
/// something else — a drag, a modal) to the demo button's tooltip:
/// hovering the button arms it, hovering the shown tooltip itself keeps
/// it open, anything else hides it. Returns whether the tooltip's node
/// appeared or went away — the caller re-runs layout (a new node has no
/// bounds until then), re-announces and redraws on `true`.
///
/// # Errors
///
/// Whatever [`Tooltip::set_hover`] refuses (the button gone).
pub fn gallery_hover(
    tree: &mut WidgetTree<WidgetKind>,
    gallery: &mut GalleryPanel,
    point: Option<(f32, f32)>,
    now: Instant,
) -> Result<bool, WidgetError> {
    let before = gallery.tooltip.node();
    let hit = point.and_then(|point| tree.hit_test(point));
    let on_tip = hit.is_some_and(|hit| before.is_some() && tree.popover_root_of(hit) == before);
    // The tooltip's node is parented under the button, so "within the
    // button" alone would count the tooltip as the owner.
    let owner = !on_tip && hit.is_some_and(|hit| tree.is_within(gallery.button, hit));
    gallery.tooltip.set_hover(tree, owner, on_tip, now)?;
    Ok(gallery.tooltip.node() != before)
}

/// Advances the demo tooltip's timer to `now` — a pending tooltip whose
/// delay has elapsed is shown. Returns whether its node appeared or went
/// away, as [`gallery_hover`].
///
/// # Errors
///
/// Whatever [`Tooltip::tick`] refuses (the button gone).
pub fn gallery_tick(
    tree: &mut WidgetTree<WidgetKind>,
    gallery: &mut GalleryPanel,
    now: Instant,
) -> Result<bool, WidgetError> {
    let before = gallery.tooltip.node();
    gallery.tooltip.tick(tree, now)?;
    Ok(gallery.tooltip.node() != before)
}

/// When the gallery next needs a [`gallery_tick`]: a pending tooltip's
/// show deadline, `None` when nothing is pending (the owner's event loop
/// can then sleep until the next real event).
#[must_use]
pub fn gallery_next_deadline(gallery: &GalleryPanel) -> Option<Instant> {
    gallery.tooltip.next_deadline()
}

fn forget_stale_menu(tree: &WidgetTree<WidgetKind>, gallery: &mut GalleryPanel) {
    if gallery.open_menu.is_some_and(|menu| !tree.contains(menu)) {
        gallery.open_menu = None;
    }
}

/// The gallery's own reaction to a routed outcome: the "Open menu"
/// button's activation opens the demo menu just below it (or closes it
/// if already open), and a menu activation or cancellation forgets the
/// menu and gives focus back to the button that opened it — the caller's
/// half of `aurora_widgets::widgets::menu`'s "the caller owns focus"
/// contract. An activated demo-tree row becomes the **only** selected row
/// (single selection — the gallery's own policy, applied here so the
/// pointer, keyboard and accessibility paths share it, and only after the
/// row's `Click` actually succeeded); a disabled row that was selected is
/// deselected too. Re-expanding the demo tree's parent after a collapse
/// rebuilds its two children and updates [`GalleryPanel::tree_rows`].
/// A press or activation of the demo button dismisses its tooltip,
/// whichever path (pointer, keyboard, assistive technology) it came by.
/// Every other outcome needs nothing from the gallery.
///
/// # Errors
///
/// Whatever `open_menu`/`close_menu` or focusing refuses.
pub fn apply_gallery_outcome(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    gallery: &mut GalleryPanel,
    scales: &Scales,
    outcome: &PointerOutcome,
) -> Result<(), WidgetError> {
    let previous = gallery.open_menu;
    forget_stale_menu(tree, gallery);
    if let PointerOutcome::Pressed(id) | PointerOutcome::Action(ActionOutcome::Activated(id)) =
        outcome
        && *id == gallery.button
    {
        gallery.tooltip.owner_pressed(tree)?;
    }
    match outcome {
        PointerOutcome::Action(ActionOutcome::Activated(id)) if *id == gallery.menu_button => {
            if let Some(menu) = gallery.open_menu.take() {
                widgets::close_menu(tree, menu)?;
                focus.validate(tree);
                return Ok(());
            }
            let (Some(button), Some(body)) = (
                tree.bounds(gallery.menu_button),
                tree.bounds(gallery.panel.body),
            ) else {
                return Ok(());
            };
            #[allow(clippy::cast_precision_loss)]
            let at = (
                button.x as f32 - body.x as f32,
                button.bottom() as f32 - body.y as f32,
            );
            let menu = widgets::open_menu(
                tree,
                gallery.panel.body,
                scales,
                "Demo menu",
                at,
                gallery_editor_size(scales),
                vec![
                    MenuItem::action("First item"),
                    MenuItem::action("Second item"),
                    MenuItem::separator(),
                    MenuItem::action("Third item"),
                ],
            )?;
            gallery.open_menu = Some(menu);
            focus.focus(tree, menu)?;
        }
        PointerOutcome::Action(ActionOutcome::MenuActivated { menu, .. })
        | PointerOutcome::MenuCancelled { menu }
            if previous == Some(*menu) =>
        {
            gallery.open_menu = None;
            focus.focus(tree, gallery.menu_button)?;
        }
        PointerOutcome::Action(ActionOutcome::Activated(row))
            if tree.is_within(gallery.tree_view, *row)
                && matches!(tree.payload(*row), Some(WidgetKind::TreeItem(_))) =>
        {
            select_only(tree, gallery.tree_view, *row)?;
        }
        PointerOutcome::Action(ActionOutcome::ExpandedChanged { id, expanded: true })
            if *id == gallery.tree_rows[0]
                && !gallery.tree_rows[1..]
                    .iter()
                    .any(|&child| tree.contains(child)) =>
        {
            let [first, second] = insert_tree_children(tree, *id, scales)?;
            gallery.tree_rows = [*id, first, second];
        }
        _ => {}
    }
    Ok(())
}

/// Selects `row` and deselects every other row under `tree_view` —
/// including a **disabled** one, which `set_tree_item_selected` refuses
/// on its own, so it is briefly re-enabled, deselected and disabled
/// again (a stale selection on a row the user cannot interact with is
/// exactly what single selection must clear).
fn select_only(
    tree: &mut WidgetTree<WidgetKind>,
    tree_view: WidgetId,
    row: WidgetId,
) -> Result<(), WidgetError> {
    let mut stack = vec![tree_view];
    let mut others = Vec::new();
    while let Some(id) = stack.pop() {
        if id != row
            && let Some(WidgetKind::TreeItem(state)) = tree.payload(id)
            && state.selected
        {
            others.push((id, state.disabled));
        }
        stack.extend(tree.children(id).unwrap_or_default().iter().copied());
    }
    for (other, disabled) in others {
        if disabled {
            widgets::set_tree_item_disabled(tree, other, false)?;
        }
        let deselected = widgets::set_tree_item_selected(tree, other, false);
        // If re-disabling fails the row is left enabled; unreachable for a
        // tree-item id this walk just found.
        if disabled {
            widgets::set_tree_item_disabled(tree, other, true)?;
        }
        deselected?;
    }
    widgets::set_tree_item_selected(tree, row, true)
}

#[cfg(test)]
mod tests {
    use accesskit::Role;
    use aurora_theme::Scales;
    use aurora_widgets::widgets::{self, WidgetKind};
    use aurora_widgets::{ActionOutcome, FocusManager, PointerOutcome, WidgetId, WidgetTree};

    use std::time::Instant;

    use aurora_widgets::widgets::TooltipPhase;

    use super::{
        GALLERY_TOOLTIP_DELAY, GalleryPanel, apply_gallery_outcome, gallery_contains,
        gallery_content_height, gallery_hover, gallery_light_dismiss, gallery_next_deadline,
        gallery_tick, gallery_width, insert_gallery_panel, remove_gallery_panel,
    };
    use crate::workspace::{Workspace, build_workspace};

    const WIDE: f32 = 2400.0;
    const TALL: f32 = 1600.0;

    fn test_scales() -> Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn opened(height: f32) -> (Workspace, GalleryPanel, Scales) {
        opened_at(WIDE, height)
    }

    fn opened_at(width: f32, height: f32) -> (Workspace, GalleryPanel, Scales) {
        let scales = test_scales();
        let mut ws = build_workspace();
        let gallery = match insert_gallery_panel(&mut ws.tree, ws.root, &scales) {
            Ok(gallery) => gallery,
            Err(err) => unreachable!("{err:?}"),
        };
        ws.tree.compute_layout(width, height);
        (ws, gallery, scales)
    }

    fn ids(g: &GalleryPanel) -> Vec<WidgetId> {
        let mut ids = vec![
            g.panel.root,
            g.button,
            g.checkbox,
            g.slider,
            g.text_field,
            g.scrollbar,
            g.swatch,
            g.dropdown,
            g.tab_bar,
            g.menu_button,
            g.tree_view,
            g.picker,
            g.curve,
        ];
        ids.extend(g.tree_rows);
        ids
    }

    fn role(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Option<Role> {
        tree.accessibility(id).map(accesskit::Node::role)
    }

    #[test]
    fn the_gallery_holds_one_accessible_widget_of_every_kind() {
        let (ws, g, _) = opened(TALL);
        assert_eq!(ws.tree.parent(g.panel.root), Some(ws.root));
        assert_eq!(
            ws.tree.children(ws.root).and_then(<[WidgetId]>::last),
            Some(&g.panel.root),
            "docked after the rail"
        );
        for (id, want) in [
            (g.button, Role::Button),
            (g.checkbox, Role::CheckBox),
            (g.slider, Role::Slider),
            (g.text_field, Role::TextInput),
            (g.scrollbar, Role::ScrollBar),
            (g.dropdown, Role::ComboBox),
            (g.tab_bar, Role::TabList),
            (g.menu_button, Role::Button),
            (g.tree_view, Role::Tree),
            (g.tree_rows[0], Role::TreeItem),
            (g.tree_rows[2], Role::TreeItem),
        ] {
            assert_eq!(role(&ws.tree, id), Some(want), "{id:?}");
        }
        for id in ids(&g) {
            assert!(ws.tree.accessibility(id).is_some(), "{id:?} has a node");
        }
        assert_eq!(g.tooltip.owner(), g.button);
        assert_eq!(g.tooltip.node(), None, "created, never shown this round");
        assert_eq!(g.open_menu, None);
    }

    #[test]
    fn a_large_viewport_lays_every_widget_out_in_two_columns_right_of_the_rail() {
        let (ws, g, scales) = opened(TALL);
        for id in ids(&g) {
            let Some(b) = ws.tree.bounds(id) else {
                unreachable!("{id:?} laid out");
            };
            assert!(b.width > 0 && b.height > 0, "{id:?} has area: {b:?}");
        }
        let (Some(panel), Some(rail), Some(left), Some(right)) = (
            ws.tree.bounds(g.panel.root),
            ws.tree.bounds(ws.rail),
            ws.tree.bounds(g.button),
            ws.tree.bounds(g.picker),
        ) else {
            unreachable!()
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let width = gallery_width(&scales) as u32;
        assert_eq!(panel.width, width, "fixed width");
        assert!(panel.x >= rail.right(), "right of the rail");
        assert!(left.right() <= right.x, "columns don't overlap");
        assert!(right.right() <= panel.right(), "inside the panel");
    }

    #[test]
    fn a_body_as_tall_as_the_content_clips_nothing_and_a_shorter_one_clips_the_tree() {
        let scales = test_scales();
        let (ws, g, _) = opened(TALL);
        let (Some(content), Some(body)) = (
            gallery_content_height(&ws.tree, &g),
            ws.tree.bounds(g.panel.body),
        ) else {
            unreachable!()
        };
        // The recorded minimum, for the default scales: every current
        // widget stacked, and well under a 1080p window's height.
        #[allow(clippy::cast_precision_loss)]
        let min = (body.y + i64::from(content)) as f32;
        eprintln!("gallery minimum window height (logical px): {min}");
        // Pinned (red-team RT-4 / critic C12): a new widget or a token
        // change that moves this must update it knowingly.
        assert!((min - 422.0).abs() < f32::EPSILON, "{min}");
        let last_row_vs_body = |height: f32| {
            let (ws, g, _) = opened(height);
            let (Some(row), Some(body)) =
                (ws.tree.bounds(g.tree_rows[2]), ws.tree.bounds(g.panel.body))
            else {
                unreachable!()
            };
            (row.bottom(), body.bottom())
        };
        let (row, bottom) = last_row_vs_body(min);
        assert!(row <= bottom, "{row} <= {bottom} at {min}");
        let (row, bottom) = last_row_vs_body(min - widgets::row_height(&scales) * 2.0);
        assert!(row > bottom, "clipped below the minimum: {row} > {bottom}");
    }

    /// Red-team RT-4 / critic C12, disclosed rather than fixed: the
    /// gallery never shrinks, so at 640 x 480 it and the rail leave the
    /// canvas a sliver. Pinned here so a regression that pushes the
    /// gallery *out of the window* (or the canvas negative) fails.
    #[test]
    fn at_640_by_480_the_gallery_stays_inside_the_window_and_the_canvas_is_a_sliver() {
        let (ws, g, _) = opened_at(640.0, 480.0);
        let (Some(panel), Some(canvas)) =
            (ws.tree.bounds(g.panel.root), ws.tree.bounds(ws.canvas_area))
        else {
            unreachable!()
        };
        eprintln!("canvas at 640x480: {canvas:?}, gallery: {panel:?}");
        assert!(panel.x >= 0 && panel.right() <= 640, "{panel:?}");
        assert!(panel.bottom() <= 480, "{panel:?}");
        assert!(canvas.x >= 0 && canvas.right() <= panel.x, "{canvas:?}");
        assert!(
            canvas.width < 64,
            "the disclosed narrow canvas — widen this pin knowingly: {canvas:?}"
        );
    }

    #[test]
    fn removing_the_gallery_leaves_none_of_its_ids_and_the_workspace_as_built() {
        let (mut ws, g, _) = opened(TALL);
        let all = ids(&g);
        let panel_root = g.panel.root;
        if let Err(err) = remove_gallery_panel(&mut ws.tree, g) {
            unreachable!("{err:?}");
        }
        for id in all {
            assert!(!ws.tree.contains(id), "{id:?} removed");
        }
        assert!(!ws.tree.contains(panel_root));
        assert_eq!(
            ws.tree.children(ws.root),
            Some([ws.canvas_area, ws.divider, ws.rail].as_slice())
        );
    }

    #[test]
    fn the_menu_button_opens_a_menu_inside_the_gallery_and_escape_hands_focus_back() {
        let (mut ws, mut g, scales) = opened(TALL);
        let mut focus = FocusManager::new();
        let activated = PointerOutcome::Action(ActionOutcome::Activated(g.menu_button));
        if let Err(err) =
            apply_gallery_outcome(&mut ws.tree, &mut focus, &mut g, &scales, &activated)
        {
            unreachable!("{err:?}");
        }
        let Some(menu) = g.open_menu else {
            unreachable!("opened");
        };
        ws.tree.compute_layout(WIDE, TALL);
        assert!(gallery_contains(&ws.tree, &g, menu));
        assert_eq!(focus.focused(), Some(menu), "the menu holds focus");
        let (Some(button), Some(bounds)) = (ws.tree.bounds(g.menu_button), ws.tree.bounds(menu))
        else {
            unreachable!()
        };
        assert_eq!(
            (bounds.x, bounds.y),
            (button.x, button.bottom()),
            "just below"
        );
        if let Err(err) = widgets::close_menu(&mut ws.tree, menu) {
            unreachable!("{err:?}");
        }
        let cancelled = PointerOutcome::MenuCancelled { menu };
        if let Err(err) =
            apply_gallery_outcome(&mut ws.tree, &mut focus, &mut g, &scales, &cancelled)
        {
            unreachable!("{err:?}");
        }
        assert_eq!(g.open_menu, None);
        assert_eq!(focus.focused(), Some(g.menu_button));
    }

    #[test]
    fn a_press_outside_an_open_menu_or_dropdown_closes_it() {
        let (mut ws, mut g, scales) = opened(TALL);
        let mut focus = FocusManager::new();
        let activated = PointerOutcome::Action(ActionOutcome::Activated(g.menu_button));
        if let Err(err) =
            apply_gallery_outcome(&mut ws.tree, &mut focus, &mut g, &scales, &activated)
        {
            unreachable!("{err:?}");
        }
        if let Err(err) = widgets::set_dropdown_open(&mut ws.tree, g.dropdown, true) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(WIDE, TALL);
        let Some(picker) = ws.tree.bounds(g.picker) else {
            unreachable!()
        };
        #[allow(clippy::cast_precision_loss)]
        let elsewhere = (picker.x as f32 + 1.0, picker.y as f32 + 1.0);
        match gallery_light_dismiss(&mut ws.tree, &mut focus, &mut g, elsewhere) {
            Ok(closed) => assert!(closed),
            Err(err) => unreachable!("{err:?}"),
        }
        assert_eq!(g.open_menu, None);
        assert!(matches!(
            widgets::dropdown_state(&ws.tree, g.dropdown).map(widgets::DropdownState::is_open),
            Ok(false)
        ));
        assert_eq!(
            focus.focused(),
            Some(g.menu_button),
            "focus inside the dismissed menu goes back to its opener (critic C11)"
        );
        match gallery_light_dismiss(&mut ws.tree, &mut focus, &mut g, elsewhere) {
            Ok(closed) => assert!(!closed, "nothing left open to close"),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn apply(ws: &mut Workspace, g: &mut GalleryPanel, scales: &Scales, outcome: &PointerOutcome) {
        let mut focus = FocusManager::new();
        if let Err(err) = apply_gallery_outcome(&mut ws.tree, &mut focus, g, scales, outcome) {
            unreachable!("{err:?}");
        }
    }

    fn selected(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> (bool, bool) {
        match tree.payload(id) {
            Some(WidgetKind::TreeItem(state)) => (state.selected, state.disabled),
            other => unreachable!("{other:?}"),
        }
    }

    /// Critic C2: single selection is the gallery's own policy, applied on
    /// `Activated` (so after a `Click` that succeeded), and it clears a
    /// selected *disabled* row too.
    #[test]
    fn an_activated_tree_row_becomes_the_only_selection_even_over_a_disabled_one() {
        let (mut ws, mut g, scales) = opened(TALL);
        let [parent, first, second] = g.tree_rows;
        for result in [
            widgets::set_tree_item_selected(&mut ws.tree, parent, true),
            widgets::set_tree_item_selected(&mut ws.tree, first, true),
            widgets::set_tree_item_disabled(&mut ws.tree, first, true),
        ] {
            if let Err(err) = result {
                unreachable!("{err:?}");
            }
        }
        apply(
            &mut ws,
            &mut g,
            &scales,
            &PointerOutcome::Action(ActionOutcome::Activated(second)),
        );
        assert_eq!(selected(&ws.tree, second), (true, false));
        assert_eq!(selected(&ws.tree, parent), (false, false));
        assert_eq!(
            selected(&ws.tree, first),
            (false, true),
            "deselected, and still disabled"
        );
        // Anything else activated selects nothing.
        let button = g.button;
        apply(
            &mut ws,
            &mut g,
            &scales,
            &PointerOutcome::Action(ActionOutcome::Activated(button)),
        );
        assert_eq!(selected(&ws.tree, second), (true, false));
    }

    /// Red-team RT-3: a collapse removes the child rows; re-expanding
    /// rebuilds them under new ids and `tree_rows` follows.
    #[test]
    fn re_expanding_the_demo_tree_rebuilds_its_children_and_tree_rows_follows() {
        let (mut ws, mut g, scales) = opened(TALL);
        let [parent, first, second] = g.tree_rows;
        if let Err(err) = widgets::set_tree_item_expanded(&mut ws.tree, parent, false) {
            unreachable!("{err:?}");
        }
        assert!(!ws.tree.contains(first) && !ws.tree.contains(second));
        apply(
            &mut ws,
            &mut g,
            &scales,
            &PointerOutcome::Action(ActionOutcome::ExpandedChanged {
                id: parent,
                expanded: false,
            }),
        );
        assert_eq!(
            g.tree_rows,
            [parent, first, second],
            "stale while collapsed"
        );
        if let Err(err) = widgets::set_tree_item_expanded(&mut ws.tree, parent, true) {
            unreachable!("{err:?}");
        }
        apply(
            &mut ws,
            &mut g,
            &scales,
            &PointerOutcome::Action(ActionOutcome::ExpandedChanged {
                id: parent,
                expanded: true,
            }),
        );
        let [same_parent, new_first, new_second] = g.tree_rows;
        assert_eq!(same_parent, parent);
        assert_ne!(new_first, first, "new ids");
        for child in [new_first, new_second] {
            assert_eq!(ws.tree.parent(child), Some(parent));
            assert_eq!(role(&ws.tree, child), Some(Role::TreeItem));
        }
        // A second expand notification with the children present adds none.
        apply(
            &mut ws,
            &mut g,
            &scales,
            &PointerOutcome::Action(ActionOutcome::ExpandedChanged {
                id: parent,
                expanded: true,
            }),
        );
        assert_eq!(g.tree_rows, [parent, new_first, new_second]);
        assert_eq!(ws.tree.children(parent).map(<[WidgetId]>::len), Some(2));
    }

    /// Critic C6: a tooltip detach that fails still removes the panel.
    #[test]
    fn a_failed_tooltip_detach_still_removes_the_panel() {
        let (mut ws, g, _) = opened(TALL);
        let root = g.panel.root;
        let button = g.button;
        if let Err(err) = ws.tree.remove(button) {
            unreachable!("{err:?}");
        }
        let result = remove_gallery_panel(&mut ws.tree, g);
        assert!(
            matches!(result, Err(aurora_widgets::WidgetError::UnknownWidget(id)) if id == button),
            "the detach error is still reported: {result:?}"
        );
        assert!(!ws.tree.contains(root), "but the panel is gone");
    }

    // -- the demo tooltip (0.131.0) --

    fn centre_of(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> (f32, f32) {
        let Some(b) = tree.bounds(id) else {
            unreachable!("{id:?} is laid out")
        };
        #[allow(clippy::cast_precision_loss)]
        let point = (
            b.x as f32 + b.width as f32 / 2.0,
            b.y as f32 + b.height as f32 / 2.0,
        );
        point
    }

    fn hover(
        ws: &mut Workspace,
        g: &mut GalleryPanel,
        point: Option<(f32, f32)>,
        now: Instant,
    ) -> bool {
        match gallery_hover(&mut ws.tree, g, point, now) {
            Ok(changed) => changed,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn tick(ws: &mut Workspace, g: &mut GalleryPanel, now: Instant) -> bool {
        match gallery_tick(&mut ws.tree, g, now) {
            Ok(changed) => changed,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// Hovers the button and ticks past the delay; returns the shown node.
    fn show(ws: &mut Workspace, g: &mut GalleryPanel, t0: Instant) -> WidgetId {
        let on_button = centre_of(&ws.tree, g.button);
        assert!(!hover(ws, g, Some(on_button), t0), "armed, not shown yet");
        assert_eq!(gallery_next_deadline(g), Some(t0 + GALLERY_TOOLTIP_DELAY));
        assert!(
            tick(ws, g, t0 + GALLERY_TOOLTIP_DELAY),
            "shown at the deadline"
        );
        ws.tree.compute_layout(WIDE, TALL);
        let Some(node) = g.tooltip.node() else {
            unreachable!("shown")
        };
        node
    }

    #[test]
    fn hovering_the_button_arms_and_tick_shows_the_tooltip() {
        let (mut ws, mut g, _) = opened(TALL);
        let t0 = Instant::now();
        assert_eq!(gallery_next_deadline(&g), None, "nothing pending");
        let on_button = centre_of(&ws.tree, g.button);
        hover(&mut ws, &mut g, Some(on_button), t0);
        assert!(!tick(&mut ws, &mut g, t0), "too early");
        assert!(g.tooltip.node().is_none());
        let node = show(&mut ws, &mut g, t0);
        assert_eq!(g.tooltip.phase(), TooltipPhase::Shown);
        assert!(gallery_contains(&ws.tree, &g, node), "inside the gallery");
        assert_eq!(gallery_next_deadline(&g), None, "nothing left pending");
        assert!(
            !hover(
                &mut ws,
                &mut g,
                Some(on_button),
                t0 + GALLERY_TOOLTIP_DELAY * 2
            ),
            "continued hover changes nothing"
        );
    }

    #[test]
    fn leaving_hides_it() {
        let (mut ws, mut g, _) = opened(TALL);
        let t0 = Instant::now();
        let node = show(&mut ws, &mut g, t0);
        let elsewhere = centre_of(&ws.tree, g.slider);
        assert!(hover(
            &mut ws,
            &mut g,
            Some(elsewhere),
            t0 + GALLERY_TOOLTIP_DELAY
        ));
        assert!(!ws.tree.contains(node));
        assert_eq!(g.tooltip.phase(), TooltipPhase::Idle);
        // Off the window entirely hides it too.
        show(&mut ws, &mut g, t0 + GALLERY_TOOLTIP_DELAY * 4);
        assert!(hover(&mut ws, &mut g, None, t0 + GALLERY_TOOLTIP_DELAY * 6));
        assert!(g.tooltip.node().is_none());
    }

    #[test]
    fn moving_onto_the_shown_tooltip_keeps_it() {
        let (mut ws, mut g, _) = opened(TALL);
        let t0 = Instant::now();
        let node = show(&mut ws, &mut g, t0);
        let on_tip = centre_of(&ws.tree, node);
        assert_eq!(
            ws.tree
                .hit_test(on_tip)
                .and_then(|hit| ws.tree.popover_root_of(hit)),
            Some(node),
            "the tooltip is hittable"
        );
        assert!(!hover(
            &mut ws,
            &mut g,
            Some(on_tip),
            t0 + GALLERY_TOOLTIP_DELAY * 2
        ));
        assert_eq!(g.tooltip.node(), Some(node), "kept open");
        assert_eq!(g.tooltip.phase(), TooltipPhase::Shown);
    }

    #[test]
    fn pressing_the_button_dismisses_it() {
        let (mut ws, mut g, scales) = opened(TALL);
        let t0 = Instant::now();
        let node = show(&mut ws, &mut g, t0);
        let mut focus = FocusManager::new();
        let pressed = PointerOutcome::Pressed(g.button);
        if let Err(err) = apply_gallery_outcome(&mut ws.tree, &mut focus, &mut g, &scales, &pressed)
        {
            unreachable!("{err:?}");
        }
        assert!(!ws.tree.contains(node), "hidden by the press");
        assert_eq!(g.tooltip.phase(), TooltipPhase::Dismissed);
        // Still hovering the button: dismissed is sticky until a fresh rise.
        let on_button = centre_of(&ws.tree, g.button);
        hover(
            &mut ws,
            &mut g,
            Some(on_button),
            t0 + GALLERY_TOOLTIP_DELAY * 2,
        );
        assert_eq!(gallery_next_deadline(&g), None);
        // A keyboard or assistive-technology activation dismisses too.
        hover(&mut ws, &mut g, None, t0 + GALLERY_TOOLTIP_DELAY * 3);
        show(&mut ws, &mut g, t0 + GALLERY_TOOLTIP_DELAY * 4);
        let activated = PointerOutcome::Action(ActionOutcome::Activated(g.button));
        if let Err(err) =
            apply_gallery_outcome(&mut ws.tree, &mut focus, &mut g, &scales, &activated)
        {
            unreachable!("{err:?}");
        }
        assert!(g.tooltip.node().is_none());
    }

    #[test]
    fn removing_the_gallery_with_a_shown_tooltip_leaves_no_node() {
        let (mut ws, mut g, _) = opened(TALL);
        let node = show(&mut ws, &mut g, Instant::now());
        if let Err(err) = remove_gallery_panel(&mut ws.tree, g) {
            unreachable!("{err:?}");
        }
        assert!(!ws.tree.contains(node));
        assert_eq!(
            ws.tree.children(ws.root),
            Some([ws.canvas_area, ws.divider, ws.rail].as_slice())
        );
    }
}
