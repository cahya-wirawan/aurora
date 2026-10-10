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
//! **Overflow: the tabs shrink.** Each tab takes an equal share of the
//! strip (the toolkit's `flex_grow: 1`, `flex_basis: 0` tab style) and its
//! label ends in "…" when it no longer fits (the paint's
//! `TextOverflow::Ellipsis` for tab labels). The toolkit has only
//! vertical scroll containers, so a scrolling strip would be new toolkit
//! work; shrinking keeps every open document visible and clickable at the
//! cost of legibility past a dozen or so documents.
//!
//! What a tab *does* lives in `aurora-app`: any selection change on the
//! strip (pointer, arrow keys, an assistive technology's `Click`) is
//! followed by the same document switch `Ctrl+Tab` runs.

use accesskit::Role;
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::TaffyZero as _;

/// The strip's accessible label.
pub const DOCUMENT_TABS_LABEL: &str = "Documents";

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
) -> Result<WidgetId, WidgetError> {
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
    tree.set_style(bar, style)?;
    Ok(bar)
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
