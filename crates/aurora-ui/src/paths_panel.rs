//! The Paths panel (0.233.0): the document's Bezier paths as a list.
//!
//! **What it shows.** One flat tree row per path, in the order the caller
//! gives (saved paths, then the unsaved "Work Path" like any other name);
//! with none, a single non-interactive "No paths" label. A toolbar at the
//! panel's foot has three icon buttons: Make Selection from Path, New Path
//! and Delete Path.
//!
//! **Selection and activation.** A press on a row selects it (making it the
//! Pen's active path: that is `aurora-app`'s call); this module only builds
//! the rows and answers which row a widget is. Make Selection and Delete
//! are disabled while no path is selected (their descriptions say why) and
//! Make Selection also while the selected path has no anchor.
//!
//! **Not here (round 1).** No thumbnails, no eye, no drag-reordering, no
//! rename in place (the palette's Rename Path prompt does it).
//!
//! The rows are *rebuilt* when the names change ([`sync_paths_panel`]) and
//! only re-highlighted when the selection changes, so a row's id and focus
//! survive a selection.

use accesskit::{Node, Orientation, Role};
use aurora_theme::Scales;
use aurora_widgets::icons::IconKind;
use aurora_widgets::widgets::{
    self, IconSize, WidgetKind, insert_tree_item, insert_tree_view, set_tree_item_description,
    set_tree_item_flat, set_tree_item_selected,
};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::{TaffyZero as _, length, percent};
use taffy::{
    AlignItems, Dimension, FlexDirection, JustifyContent, Rect as LayoutRect, Size, Style,
};

use crate::panel::{PanelHandle, clear_panel_body};

/// The toolbar's accessible name.
pub const PATHS_TOOLBAR_LABEL: &str = "Path actions";

/// The empty state's text.
pub const NO_PATHS_TEXT: &str = "No paths";

/// The icon size the toolbar's buttons use (the Layers toolbar's).
pub const PATHS_TOOLBAR_ICON_SIZE: IconSize = IconSize::Large;

/// Why Fill Path is unavailable with no path selected.
pub const FILL_UNAVAILABLE: &str = "Select a path to fill it";
/// Why Fill Path is unavailable for a path with no area.
pub const FILL_TOO_SMALL: &str = "Fill Path needs a subpath of at least three points";
/// Why Stroke Path is unavailable with no path selected.
pub const STROKE_UNAVAILABLE: &str = "Select a path to stroke it";
/// Why Stroke Path is unavailable for a path with no line.
pub const STROKE_TOO_SMALL: &str = "Stroke Path needs a subpath of at least two points";
/// Why Make Work Path is unavailable with nothing selected.
pub const MAKE_PATH_UNAVAILABLE: &str = "Make a selection to turn it into a work path";

/// Why Make Selection is unavailable with no path selected.
pub const MAKE_SELECTION_UNAVAILABLE: &str = "Select a path to make a selection from it";
/// Why Make Selection is unavailable for an empty path.
pub const MAKE_SELECTION_EMPTY: &str = "The selected path has no points yet";
/// Why Delete is unavailable.
pub const PATH_DELETE_UNAVAILABLE: &str = "Select a path to delete it";

/// What a toolbar button asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathsAction {
    /// Replace the selection with the selected path's outline.
    MakeSelection,
    /// Fill the selected path with the foreground colour (0.234.0).
    FillPath,
    /// Stroke the selected path with the Brush (0.234.0).
    StrokePath,
    /// Replace the work path with the selection's outline (0.234.0).
    MakeWorkPath,
    /// Start a new, empty path.
    NewPath,
    /// Delete the selected path.
    DeletePath,
}

impl PathsAction {
    /// Every action, in toolbar order.
    pub const ALL: [Self; 6] = [
        Self::MakeSelection,
        Self::FillPath,
        Self::StrokePath,
        Self::MakeWorkPath,
        Self::NewPath,
        Self::DeletePath,
    ];

    /// The button's accessible name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MakeSelection => "Make selection from path",
            Self::FillPath => "Fill path",
            Self::StrokePath => "Stroke path",
            Self::MakeWorkPath => "Make work path from selection",
            Self::NewPath => "New path",
            Self::DeletePath => "Delete path",
        }
    }

    /// The icon on the button (existing icons: there is no dedicated
    /// path glyph for these yet).
    #[must_use]
    pub const fn icon(self) -> IconKind {
        match self {
            Self::MakeSelection => IconKind::Marquee,
            Self::FillPath => IconKind::PaintBucket,
            Self::StrokePath => IconKind::Brush,
            Self::MakeWorkPath => IconKind::Pen,
            Self::NewPath => IconKind::NewLayer,
            Self::DeletePath => IconKind::Delete,
        }
    }

    /// The tooltip text and accessible description while available.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::MakeSelection => "Replace the selection with the outline of the selected path",
            Self::FillPath => "Fill the selected path with the foreground colour",
            Self::StrokePath => "Stroke the selected path with the Brush",
            Self::MakeWorkPath => "Replace the work path with the outline of the selection",
            Self::NewPath => "Start a new empty path",
            Self::DeletePath => "Delete the selected path",
        }
    }
}

/// The toolbar and its buttons, in [`PathsAction::ALL`] order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathsToolbar {
    pub root: WidgetId,
    pub buttons: [(PathsAction, WidgetId); PathsAction::ALL.len()],
}

impl PathsToolbar {
    /// The action button `id` stands for.
    #[must_use]
    pub fn action_for(&self, id: WidgetId) -> Option<PathsAction> {
        self.buttons
            .iter()
            .find(|(_, button)| *button == id)
            .map(|(action, _)| *action)
    }

    /// `action`'s button.
    #[must_use]
    pub fn button_for(&self, action: PathsAction) -> Option<WidgetId> {
        self.buttons
            .iter()
            .find(|(candidate, _)| *candidate == action)
            .map(|(_, button)| *button)
    }
}

/// What the toolbar needs to know to enable its buttons, beyond which path
/// is selected (the caller keeps it current through [`sync_paths_panel`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct PathsFacts {
    /// The selected path has no anchor (Make Selection is then disabled).
    pub selected_empty: bool,
    /// The selected path has a subpath Fill Path can fill.
    pub can_fill: bool,
    /// The selected path has a subpath Stroke Path can stroke.
    pub can_stroke: bool,
    /// The document has a selection to turn into a path.
    pub has_selection: bool,
}

/// The panel's live state: the toolbar, the rows as built (in screen
/// order), the names they were built for and the selected row.
#[derive(Debug, Clone, PartialEq)]
pub struct PathsPanel {
    pub toolbar: PathsToolbar,
    /// Every row, top to bottom, with its path's name.
    pub rows: Vec<(WidgetId, String)>,
    /// The names the rows were built for.
    pub names: Vec<String>,
    /// The selected path's name, if one is (and shown).
    pub selected: Option<String>,
    /// What the toolbar's enabled states depend on.
    pub facts: PathsFacts,
}

impl PathsPanel {
    /// The id of the row of the path `name`, if shown.
    #[must_use]
    pub fn id_of(&self, name: &str) -> Option<WidgetId> {
        self.rows.iter().find(|(_, n)| n == name).map(|(id, _)| *id)
    }

    /// The path name of the row `id`, when it is one of the rows.
    #[must_use]
    pub fn name_of(&self, id: WidgetId) -> Option<&str> {
        self.rows
            .iter()
            .find(|(row, _)| *row == id)
            .map(|(_, name)| name.as_str())
    }

    /// Which path's row `hit` is: `hit` itself or the row it lies in.
    #[must_use]
    pub fn row_at(&self, tree: &WidgetTree<WidgetKind>, hit: WidgetId) -> Option<&str> {
        let mut current = Some(hit);
        while let Some(id) = current {
            if let Some(name) = self.name_of(id) {
                return Some(name);
            }
            current = tree.parent(id);
        }
        None
    }

    /// The row above (`forward == false`) or below `from`, if any.
    #[must_use]
    pub fn neighbor(&self, from: WidgetId, forward: bool) -> Option<WidgetId> {
        let at = self.rows.iter().position(|(id, _)| *id == from)?;
        let next = if forward {
            at.checked_add(1)?
        } else {
            at.checked_sub(1)?
        };
        self.rows.get(next).map(|(id, _)| *id)
    }
}

#[allow(clippy::cast_precision_loss)]
fn px(token: u32) -> f32 {
    token as f32
}

/// The toolbar row's height: a button plus `spacing.xxs` above and below.
#[must_use]
pub fn paths_toolbar_height(scales: &Scales) -> f32 {
    widgets::icon_button_side(scales, PATHS_TOOLBAR_ICON_SIZE) + 2.0 * px(scales.spacing.xxs)
}

fn toolbar_style(scales: &Scales) -> Style {
    let pad = length(px(scales.spacing.xxs));
    Style {
        flex_direction: FlexDirection::Row,
        justify_content: Some(JustifyContent::FLEX_END),
        align_items: Some(AlignItems::CENTER),
        flex_shrink: 0.0,
        size: Size {
            width: Dimension::auto(),
            height: length(paths_toolbar_height(scales)),
        },
        min_size: Size {
            width: Dimension::ZERO,
            height: length(paths_toolbar_height(scales)),
        },
        gap: Size {
            width: taffy::style_helpers::zero(),
            height: taffy::style_helpers::zero(),
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

/// Adds the toolbar to `panel` (the Paths panel) as the last child of its
/// root and refreshes the panel's height floor. Make Selection and Delete
/// start disabled.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `panel.root` doesn't exist; a partly
/// built toolbar is removed again.
pub fn insert_paths_toolbar(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
) -> Result<PathsToolbar, WidgetError> {
    let mut node = Node::new(Role::Toolbar);
    node.set_label(PATHS_TOOLBAR_LABEL);
    node.set_orientation(Orientation::Horizontal);
    let root = tree.insert(
        panel.root,
        toolbar_style(scales),
        node,
        WidgetKind::Container,
    )?;
    let built = build_toolbar(tree, root, scales)
        .and_then(|toolbar| crate::panel::refresh_panel_floor(tree, panel).map(|()| toolbar));
    if built.is_err() {
        let _ = tree.remove(root);
    }
    built
}

fn build_toolbar(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    scales: &Scales,
) -> Result<PathsToolbar, WidgetError> {
    let mut buttons = [(PathsAction::MakeSelection, root); PathsAction::ALL.len()];
    for (slot, action) in buttons.iter_mut().zip(PathsAction::ALL) {
        let id = widgets::insert_icon_button(
            tree,
            root,
            scales,
            action.name(),
            action.icon(),
            PATHS_TOOLBAR_ICON_SIZE,
        )?;
        let text = match action {
            PathsAction::MakeSelection => {
                widgets::set_button_disabled(tree, id, true)?;
                MAKE_SELECTION_UNAVAILABLE
            }
            PathsAction::DeletePath => {
                widgets::set_button_disabled(tree, id, true)?;
                PATH_DELETE_UNAVAILABLE
            }
            PathsAction::FillPath => {
                widgets::set_button_disabled(tree, id, true)?;
                FILL_UNAVAILABLE
            }
            PathsAction::StrokePath => {
                widgets::set_button_disabled(tree, id, true)?;
                STROKE_UNAVAILABLE
            }
            PathsAction::MakeWorkPath => {
                widgets::set_button_disabled(tree, id, true)?;
                MAKE_PATH_UNAVAILABLE
            }
            PathsAction::NewPath => action.description(),
        };
        widgets::set_button_description(tree, id, Some(text))?;
        *slot = (action, id);
    }
    Ok(PathsToolbar { root, buttons })
}

fn set_enabled(
    tree: &mut WidgetTree<WidgetKind>,
    button: WidgetId,
    enabled: bool,
    available: &str,
    unavailable: &str,
) -> Result<bool, WidgetError> {
    let was_disabled = matches!(
        tree.payload(button),
        Some(WidgetKind::Button(state)) if state.disabled
    );
    let mut changed = false;
    if was_disabled == enabled {
        widgets::set_button_disabled(tree, button, !enabled)?;
        changed = true;
    }
    let text = if enabled { available } else { unavailable };
    changed |= widgets::set_button_description(tree, button, Some(text))?;
    Ok(changed)
}

/// Mirrors the selection into the toolbar: Delete is enabled exactly when
/// a path is selected; Make Selection when one is selected and has an
/// anchor; Fill and Stroke when its subpaths are long enough; Make Work Path
/// when the document has a selection (the descriptions say why not
/// otherwise). Returns whether
/// anything changed.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`] for a
/// missing or non-button widget.
pub fn sync_paths_toolbar(
    tree: &mut WidgetTree<WidgetKind>,
    toolbar: &PathsToolbar,
    has_selected_path: bool,
    facts: PathsFacts,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    if let Some(button) = toolbar.button_for(PathsAction::MakeSelection) {
        let reason = if has_selected_path {
            MAKE_SELECTION_EMPTY
        } else {
            MAKE_SELECTION_UNAVAILABLE
        };
        changed |= set_enabled(
            tree,
            button,
            has_selected_path && !facts.selected_empty,
            PathsAction::MakeSelection.description(),
            reason,
        )?;
    }
    if let Some(button) = toolbar.button_for(PathsAction::FillPath) {
        let reason = if has_selected_path {
            FILL_TOO_SMALL
        } else {
            FILL_UNAVAILABLE
        };
        changed |= set_enabled(
            tree,
            button,
            has_selected_path && facts.can_fill,
            PathsAction::FillPath.description(),
            reason,
        )?;
    }
    if let Some(button) = toolbar.button_for(PathsAction::StrokePath) {
        let reason = if has_selected_path {
            STROKE_TOO_SMALL
        } else {
            STROKE_UNAVAILABLE
        };
        changed |= set_enabled(
            tree,
            button,
            has_selected_path && facts.can_stroke,
            PathsAction::StrokePath.description(),
            reason,
        )?;
    }
    if let Some(button) = toolbar.button_for(PathsAction::MakeWorkPath) {
        changed |= set_enabled(
            tree,
            button,
            facts.has_selection,
            PathsAction::MakeWorkPath.description(),
            MAKE_PATH_UNAVAILABLE,
        )?;
    }
    if let Some(button) = toolbar.button_for(PathsAction::DeletePath) {
        changed |= set_enabled(
            tree,
            button,
            has_selected_path,
            PathsAction::DeletePath.description(),
            PATH_DELETE_UNAVAILABLE,
        )?;
    }
    Ok(changed)
}

/// Points the hover tint at `hovered` (a button of this toolbar); `None`
/// clears it. Returns whether anything changed.
///
/// # Errors
///
/// As [`sync_paths_toolbar`].
pub fn set_paths_toolbar_hover(
    tree: &mut WidgetTree<WidgetKind>,
    toolbar: &PathsToolbar,
    hovered: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    for (_, id) in toolbar.buttons {
        changed |= widgets::set_button_hovered(tree, id, hovered == Some(id))?;
    }
    Ok(changed)
}

/// Whether `id` is one of the Path actions toolbar's buttons (read off the
/// tree: an icon button whose parent is the labelled toolbar).
#[must_use]
pub fn is_paths_toolbar_button(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    matches!(tree.payload(id), Some(WidgetKind::Button(state)) if state.icon.is_some())
        && tree
            .parent(id)
            .and_then(|parent| tree.accessibility(parent))
            .is_some_and(|node| {
                node.role() == Role::Toolbar && node.label() == Some(PATHS_TOOLBAR_LABEL)
            })
}

/// Builds the panel's content state with an empty row list; the caller
/// then runs [`populate_paths_panel`].
#[must_use]
pub fn new_paths_panel(toolbar: PathsToolbar) -> PathsPanel {
    PathsPanel {
        toolbar,
        rows: Vec::new(),
        names: Vec::new(),
        selected: None,
        facts: PathsFacts::default(),
    }
}

fn tree_container_style() -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        flex_shrink: 0.0,
        size: Size {
            width: percent(1.0_f32),
            height: taffy::style_helpers::auto(),
        },
        ..Default::default()
    }
}

/// Rebuilds the panel's rows for the path `names`: a row per name or the
/// empty-state label. Keeps `state.selected` when it still exists, else
/// nothing is selected; mirrors the selection into the toolbar. Safe to
/// repeat.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if the panel's body is missing.
pub fn populate_paths_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
    state: &mut PathsPanel,
    names: &[String],
) -> Result<(), WidgetError> {
    clear_panel_body(tree, panel.body)?;
    let mut rows = Vec::new();
    if names.is_empty() {
        let label = widgets::insert_label(tree, panel.body, scales, NO_PATHS_TEXT)?;
        let mut style = tree.style(label).cloned().unwrap_or_default();
        let pad = length(px(scales.spacing.sm));
        style.padding = LayoutRect {
            left: pad,
            right: pad,
            top: pad,
            bottom: pad,
        };
        tree.set_style(label, style)?;
    } else {
        let list = insert_tree_view(tree, panel.body, None)?;
        tree.set_style(list, tree_container_style())?;
        for name in names {
            let row = insert_tree_item(tree, list, scales, name, false)?;
            set_tree_item_flat(tree, row, scales.spacing.sm)?;
            set_tree_item_description(
                tree,
                row,
                Some("Path. Select it to draw on it with the Pen tool."),
            )?;
            rows.push((row, name.clone()));
        }
    }
    state.rows = rows;
    state.names = names.to_vec();
    let selected = state.selected.clone();
    select_path_row(tree, state, selected.as_deref())
}

/// Makes the row of the path `name` the selected one (`None`, or a name
/// that is not shown, selects nothing): highlights it and mirrors the
/// selection into the toolbar.
///
/// # Errors
///
/// As the widget setters it calls.
pub fn select_path_row(
    tree: &mut WidgetTree<WidgetKind>,
    state: &mut PathsPanel,
    name: Option<&str>,
) -> Result<(), WidgetError> {
    let wanted = name.filter(|n| state.id_of(n).is_some()).map(str::to_owned);
    for (id, row_name) in state.rows.clone() {
        set_tree_item_selected(tree, id, wanted.as_deref() == Some(row_name.as_str()))?;
    }
    state.selected = wanted;
    sync_paths_toolbar(tree, &state.toolbar, state.selected.is_some(), state.facts)?;
    Ok(())
}

/// Keeps the panel current: rebuilds the rows when the path `names`
/// changed since they were built (or the rows died with a closed panel),
/// else only moves the highlight when `selected` changed, and always
/// mirrors `facts` into the toolbar. Cheap when nothing changed,
/// so a caller runs it every event-loop turn. Returns whether anything
/// changed (so a caller can re-lay-out).
///
/// # Errors
///
/// As [`populate_paths_panel`] / [`select_path_row`].
pub fn sync_paths_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
    state: &mut PathsPanel,
    names: &[String],
    selected: Option<&str>,
    facts: PathsFacts,
) -> Result<bool, WidgetError> {
    // A closed panel's body is emptied (`close_panel`): the remembered rows
    // (or the empty-state label) are gone and are rebuilt like a name change.
    let dead = tree.children(panel.body).is_none_or(<[WidgetId]>::is_empty)
        || state
            .rows
            .first()
            .is_some_and(|(id, _)| !tree.contains(*id));
    let facts_changed = state.facts != facts;
    state.facts = facts;
    if dead || state.names != names {
        state.selected = selected.map(str::to_owned);
        populate_paths_panel(tree, panel, scales, state, names)?;
        return Ok(true);
    }
    if state.selected.as_deref() != selected {
        select_path_row(tree, state, selected)?;
        return Ok(true);
    }
    if facts_changed {
        return sync_paths_toolbar(tree, &state.toolbar, state.selected.is_some(), facts);
    }
    Ok(false)
}

#[cfg(test)]
#[path = "paths_panel_tests.rs"]
mod tests;
