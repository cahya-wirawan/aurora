//! The document tab strip (0.173.0, document tabs round R4): one row at
//! the top of the canvas column, under the options bar and above the
//! canvas, listing every open document in tab order with the active one
//! selected.
//!
//! It is the toolkit's own [`widgets::insert_tab_bar`] — a `Role::TabList`
//! labelled [`DOCUMENT_TABS_LABEL`], one `Role::Tab` per document named
//! after it — relabelled in place by [`sync_document_tabs`]
//! ([`widgets::set_tab_labels`], which keeps surviving tabs' ids, so focus
//! resting on the strip survives a switch). The canvas area is the strip's
//! `Role::TabPanel`, `labelled_by` the selected tab, the same pattern as
//! the Properties + History group (0.164.0, [`crate::panel_group`]).
//!
//! **Overflow (0.260.0): a window of tabs and a "More documents" menu.**
//! Each tab has a readable floor ([`document_tab_min_width`]) and a
//! ceiling ([`document_tab_max_width`]), both derived from the spacing
//! scale. While every tab fits at its floor the strip shows them all,
//! sharing the width (up to the ceiling). When they do not, the strip
//! shows a **window** of consecutive tabs
//! ([`document_tab_window`]) that always contains the active document's
//! tab, moved by the smallest amount when the active document changes,
//! plus an icon button at the strip's end ("More documents", a chevron
//! down, described "N more") that opens the toolkit's `Menu` popup
//! listing **every** open document ([`open_document_overflow_menu`]);
//! `aurora-app` switches to the chosen one through the same
//! `switch_document` as `Ctrl+Tab`. Tabs outside the window keep their
//! widgets, ids and indices (so the strip's selection index is still the
//! document's index) but are `Display::None` and hidden from assistive
//! technology ([`widgets::set_tab_window`]). The toolkit has only
//! vertical scroll containers, so nothing here scrolls horizontally.
//! The window is recomputed from the laid-out strip width by
//! [`sync_document_tab_window`], which the app calls after each layout
//! pass (a strip not yet laid out shows every tab).
//!
//! What a tab *does* lives in `aurora-app`: any selection change on the
//! strip (pointer, arrow keys, an assistive technology's `Click`) is
//! followed by the same document switch `Ctrl+Tab` runs.

use accesskit::Role;
use aurora_theme::Scales;
use aurora_widgets::icons::IconKind;
use aurora_widgets::widgets::{self, IconSize, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::TaffyZero as _;

/// The strip's accessible label.
pub const DOCUMENT_TABS_LABEL: &str = "Documents";

/// The overflow button's accessible name (0.260.0).
pub const DOCUMENT_OVERFLOW_LABEL: &str = "More documents";

/// The one tab [`crate::build_workspace`] starts with, before `aurora-app`
/// names the startup document.
pub const INITIAL_DOCUMENT_TAB: &str = "Untitled";

/// Inserts the strip into `parent` (the canvas column). It keeps its one
/// row whenever the canvas column has room for it: the canvas area, whose
/// basis is zero, gives up its height first. Only in a window too short for
/// the options bar, the strip and the status bar together does the strip
/// shrink (its minimum height is zero, so the status bar stays on the
/// window's bottom edge, as since 0.162.0); its tabs then overhang it.
///
/// # Errors
///
/// A [`WidgetError`] when an id is not in the tree or is the wrong kind.
pub fn insert_document_tabs(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
) -> Result<(WidgetId, WidgetId), WidgetError> {
    let bar = widgets::insert_tab_bar(
        tree,
        parent,
        scales,
        DOCUMENT_TABS_LABEL,
        vec![INITIAL_DOCUMENT_TAB.to_owned()],
        0,
    )?;
    let mut style = tree
        .style(bar)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(bar))?;
    style.min_size.height = taffy::Dimension::ZERO;
    // The bar never asks for more width than the column gives it: tabs
    // are floored, so a long list would otherwise push its own box wider
    // than its parent (0.260.0).
    style.min_size.width = taffy::Dimension::ZERO;
    tree.set_style(bar, style)?;
    // 0.175.0: every document tab has a "×" close button.
    let _ = widgets::set_tab_bar_closable(tree, bar, true)?;
    let _ = widgets::set_tab_width_limits(
        tree,
        bar,
        Some((document_tab_min_width(scales), document_tab_max_width(scales))),
    )?;
    let overflow = widgets::insert_icon_button(
        tree,
        bar,
        scales,
        DOCUMENT_OVERFLOW_LABEL,
        IconKind::ChevronDown,
        IconSize::Small,
    )?;
    let mut button_style = tree
        .style(overflow)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(overflow))?;
    button_style.align_self = Some(taffy::AlignItems::CENTER);
    button_style.flex_shrink = 0.0;
    tree.set_style(overflow, button_style)?;
    crate::workspace::set_shown(tree, overflow, false)?;
    Ok((bar, overflow))
}

/// A document tab's width floor (0.260.0): `spacing.xxxl + spacing.xxl`
/// (112 logical px at the default density) — the tab's own padding and
/// close button (`spacing.sm` + `spacing.sm` + one control row) plus
/// room for about eight characters of the UI text. Never below the close
/// button's width plus its padding (a test pins it).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn document_tab_min_width(scales: &Scales) -> f32 {
    (scales.spacing.xxxl + scales.spacing.xxl) as f32
}

/// A document tab's width ceiling (0.260.0): `3 * spacing.xxxl` (192), so
/// a few documents do not stretch tabs across the whole strip.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn document_tab_max_width(scales: &Scales) -> f32 {
    (3 * scales.spacing.xxxl) as f32
}

/// How many tabs of width `min_width` fit in `available` logical px with
/// `gap` between them — at least one, so a strip too narrow for any (or
/// not finite) still shows the active document's tab.
#[must_use]
pub fn document_tab_capacity(available: f32, min_width: f32, gap: f32) -> usize {
    if !(available.is_finite() && min_width.is_finite() && min_width > 0.0) {
        return 1;
    }
    let gap = if gap.is_finite() { gap.max(0.0) } else { 0.0 };
    let fit = ((available.max(0.0) + gap) / (min_width + gap)).floor();
    // `fit` is a small non-negative whole number.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fit = fit as usize;
    fit.max(1)
}

/// The window of tabs (a half-open index range) to show when `count` tabs
/// do not all fit and only `capacity` can be shown: `previous` kept when
/// it already contains `selected`, otherwise moved by the smallest amount
/// that does (the selected tab becomes the first shown when it lay before
/// the window, the last when it lay after), and always inside `0..count`.
/// Every tab fits (`capacity >= count`): the whole range.
#[must_use]
pub fn document_tab_window(
    count: usize,
    selected: usize,
    capacity: usize,
    previous: Option<(usize, usize)>,
) -> (usize, usize) {
    let capacity = capacity.max(1);
    if capacity >= count {
        return (0, count);
    }
    let selected = selected.min(count.saturating_sub(1));
    let mut start = previous.map_or(0, |(start, _)| start);
    if selected < start {
        start = selected;
    } else if selected >= start.saturating_add(capacity) {
        start = selected + 1 - capacity;
    }
    start = start.min(count - capacity);
    (start, start + capacity)
}

/// Shows the window of tabs that fits the strip's laid-out width, with the
/// active tab inside it, and the overflow button when some tabs are left
/// out; returns whether anything changed (the caller then lays out again).
/// A strip that has not been laid out yet (no bounds) is left alone: every
/// tab shown, no button.
///
/// # Errors
///
/// A [`WidgetError`] for a malformed workspace (an id not in the tree).
#[allow(clippy::cast_precision_loss)]
pub fn sync_document_tab_window(
    workspace: &mut crate::Workspace,
    scales: &Scales,
) -> Result<bool, WidgetError> {
    let (bar, overflow) = (workspace.document_tabs, workspace.document_overflow);
    let tree = &mut workspace.tree;
    let (count, selected, previous) = {
        let state = widgets::tab_bar_state(tree, bar)?;
        (state.labels().len(), state.selected(), state.window())
    };
    // A strip with no width yet (never laid out, or squeezed to nothing)
    // keeps what it shows.
    let Some(width) = tree
        .bounds(bar)
        .map(|bounds| bounds.width as f32)
        .filter(|width| *width > 0.0)
    else {
        return Ok(false);
    };
    let gap = scales.spacing.xs as f32;
    let min_width = document_tab_min_width(scales);
    let all_fit = count as f32 * min_width + count.saturating_sub(1) as f32 * gap <= width;
    // The ordering fix: tabs added after the button land behind it.
    let mut changed = move_overflow_last(tree, bar, overflow)?;
    if all_fit {
        changed |= widgets::set_tab_window(tree, bar, None)?;
        changed |= show_overflow(tree, overflow, false, 0)?;
        return Ok(changed);
    }
    let button = widgets::icon_button_side(scales, IconSize::Small);
    let capacity = document_tab_capacity(width - button - gap, min_width, gap);
    let (start, end) = document_tab_window(count, selected, capacity, previous);
    changed |= if end - start >= count {
        widgets::set_tab_window(tree, bar, None)?
    } else {
        widgets::set_tab_window(tree, bar, Some((start, end)))?
    };
    changed |= show_overflow(tree, overflow, end - start < count, count - (end - start))?;
    Ok(changed)
}

/// Keeps the overflow button behind every tab (tabs the strip adds are
/// appended after it).
fn move_overflow_last(
    tree: &mut WidgetTree<WidgetKind>,
    bar: WidgetId,
    overflow: WidgetId,
) -> Result<bool, WidgetError> {
    let children = tree.children(bar).unwrap_or_default().to_vec();
    let last_tab = children
        .iter()
        .rposition(|&child| matches!(tree.payload(child), Some(WidgetKind::Tab(_))));
    let button = children.iter().position(|&child| child == overflow);
    match (last_tab, button) {
        (Some(tab), Some(button)) if button < tab => {
            tree.move_child(overflow, bar, tab)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Shows or hides the overflow button and words its description
/// ("N more").
fn show_overflow(
    tree: &mut WidgetTree<WidgetKind>,
    overflow: WidgetId,
    shown: bool,
    hidden_tabs: usize,
) -> Result<bool, WidgetError> {
    let description = shown.then(|| format!("{hidden_tabs} more"));
    // The description is written first: a button rewrites its whole node,
    // and the hidden flag below is applied on top.
    let mut changed = widgets::set_button_description(tree, overflow, description.as_deref())?;
    let before = tree.style(overflow).map(|style| style.display);
    crate::workspace::set_shown(tree, overflow, shown)?;
    changed |= before != tree.style(overflow).map(|style| style.display);
    Ok(changed)
}

/// Whether `id` is the overflow button or lies inside its popup menu (a
/// `Menu` child of the strip): the tree-only form `aurora-app`'s
/// widget-owner lookup uses.
#[must_use]
pub fn is_document_overflow_part(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    let in_strip = |bar: Option<WidgetId>| {
        bar.is_some_and(|bar| {
            tree.accessibility(bar).is_some_and(|node| {
                node.role() == Role::TabList && node.label() == Some(DOCUMENT_TABS_LABEL)
            })
        })
    };
    let mut current = Some(id);
    while let Some(node) = current {
        let parent = tree.parent(node);
        if in_strip(parent) {
            return match tree.payload(node) {
                Some(WidgetKind::Menu(_)) => true,
                Some(WidgetKind::Button(_)) => {
                    tree.accessibility(node).and_then(accesskit::Node::label)
                        == Some(DOCUMENT_OVERFLOW_LABEL)
                }
                _ => false,
            };
        }
        current = parent;
    }
    false
}

/// Opens the overflow menu under the overflow button, right-aligned to the
/// strip's end and no wider than the strip: the toolkit's `Menu` as a
/// child of the strip, `width` wide (capped to the strip), listing
/// `items`. The caller focuses the returned id.
///
/// # Errors
///
/// A [`WidgetError`] when the strip has no layout yet, `items` has no
/// action, or the toolkit refuses the menu.
pub fn open_document_overflow_menu(
    workspace: &mut crate::Workspace,
    scales: &Scales,
    width: f32,
    items: Vec<widgets::MenuItem>,
) -> Result<WidgetId, WidgetError> {
    let (bar, overflow) = (workspace.document_tabs, workspace.document_overflow);
    let tree = &mut workspace.tree;
    let (Some(bar_bounds), Some(button_bounds)) = (tree.bounds(bar), tree.bounds(overflow)) else {
        return Err(WidgetError::UnknownWidget(bar));
    };
    #[allow(clippy::cast_precision_loss)]
    let (bar_width, bar_height, right) = (
        bar_bounds.width as f32,
        bar_bounds.height as f32,
        (button_bounds.x - bar_bounds.x) as f32 + button_bounds.width as f32,
    );
    let width = if bar_width > 0.0 {
        width.min(bar_width)
    } else {
        width
    };
    let at = ((right - width).max(0.0), bar_height);
    widgets::open_menu(tree, bar, scales, DOCUMENT_OVERFLOW_LABEL, at, width, items)
}

/// Sets each document tab's accessible name (0.175.0 review P-1): the
/// document's own name, without the unsaved marker the drawn label
/// carries (the "unsaved" description says it). `Ok(false)` when unchanged.
///
/// # Errors
///
/// [`WidgetError`] if `bar` is not a tab bar.
pub fn set_document_tab_names(
    tree: &mut WidgetTree<WidgetKind>,
    bar: WidgetId,
    names: Vec<String>,
) -> Result<bool, WidgetError> {
    widgets::set_tab_accessible_names(tree, bar, names)
}

/// Which document tab's close button `id` is (0.175.0), if any — the
/// assistive-technology `Click` route.
#[must_use]
pub fn document_tab_close_index(
    tree: &WidgetTree<WidgetKind>,
    bar: WidgetId,
    id: WidgetId,
) -> Option<usize> {
    widgets::tab_close_index(tree, bar, id)
}

/// Which document tab's close button is under `point` (logical px), if
/// any — the pointer route (0.175.0).
#[must_use]
pub fn document_tab_close_at(
    tree: &WidgetTree<WidgetKind>,
    bar: WidgetId,
    point: (f32, f32),
) -> Option<usize> {
    tree.hit_test(point)
        .and_then(|hit| widgets::tab_close_index(tree, bar, hit))
}

/// Makes `panel` (the canvas area) the strip's `Role::TabPanel`, labelled
/// by the selected tab — only ever a live tab id (`accesskit_consumer`
/// unwraps every `labelled_by` id it resolves).
///
/// # Errors
///
/// A [`WidgetError`] when an id is not in the tree or is the wrong kind.
pub fn link_document_panel(
    tree: &mut WidgetTree<WidgetKind>,
    bar: WidgetId,
    panel: WidgetId,
) -> Result<(), WidgetError> {
    let selected = widgets::tab_bar_state(tree, bar)?.selected_tab();
    let mut node = tree
        .accessibility(panel)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(panel))?;
    node.set_role(Role::TabPanel);
    match selected {
        Some(tab) if tree.contains(tab) => node.set_labelled_by(vec![tab]),
        _ => node.clear_labelled_by(),
    }
    if tree.accessibility(panel) != Some(&node) {
        tree.set_accessibility(panel, node)?;
        tree.mark_dirty(panel)?;
    }
    Ok(())
}

/// Shows `labels` (every open document's name, in tab order) with
/// `selected` (the active one) and relinks the canvas panel to it.
///
/// # Errors
///
/// A [`WidgetError`] when an id is not in the tree or is the wrong kind.
pub fn sync_document_tabs(
    tree: &mut WidgetTree<WidgetKind>,
    bar: WidgetId,
    panel: WidgetId,
    labels: Vec<String>,
    selected: usize,
) -> Result<(), WidgetError> {
    let unchanged = widgets::tab_bar_state(tree, bar)
        .is_ok_and(|state| state.selected() == selected && state.labels() == labels.as_slice());
    if !unchanged {
        widgets::set_tab_labels(tree, bar, labels, selected)?;
    }
    link_document_panel(tree, bar, panel)
}

/// Sets each document tab's accessible description (0.174.0: ", unsaved"
/// state), by tab position. `Ok(false)` when nothing changed.
///
/// # Errors
///
/// [`WidgetError`] if `bar` is not a tab bar.
pub fn set_document_tab_descriptions(
    tree: &mut WidgetTree<WidgetKind>,
    bar: WidgetId,
    descriptions: Vec<String>,
) -> Result<bool, WidgetError> {
    widgets::set_tab_descriptions(tree, bar, descriptions)
}

/// Whether `id` is one of the strip's tabs.
#[must_use]
pub fn is_document_tab(tree: &WidgetTree<WidgetKind>, bar: WidgetId, id: WidgetId) -> bool {
    widgets::tab_bar_state(tree, bar).is_ok_and(|state| state.index_of(id).is_some())
}

/// The strip's selected index.
#[must_use]
pub fn document_tab_selected(tree: &WidgetTree<WidgetKind>, bar: WidgetId) -> Option<usize> {
    widgets::tab_bar_state(tree, bar)
        .ok()
        .map(widgets::TabBarState::selected)
}

/// Whether `id` is a tab of *a* document strip, from the tree alone: its
/// parent is a `Role::TabList` labelled [`DOCUMENT_TABS_LABEL`]. For
/// routing code that has the tree but not the [`crate::Workspace`].
#[must_use]
pub fn is_document_strip_tab(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    matches!(tree.payload(id), Some(WidgetKind::Tab(_)))
        && tree.parent(id).is_some_and(|bar| {
            tree.accessibility(bar).is_some_and(|node| {
                node.role() == Role::TabList && node.label() == Some(DOCUMENT_TABS_LABEL)
            })
        })
}

#[cfg(test)]
#[path = "document_tabs_tests.rs"]
mod tests;
