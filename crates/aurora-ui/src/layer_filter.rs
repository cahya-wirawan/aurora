//! The Layers panel's **filter row** (0.199.0, round L5b): a name search
//! field and three kind toggles at the top of the panel, above the blend
//! and Opacity strip, as in the reference.
//!
//! **Semantics.** A layer *matches* when its (display) name contains the
//! search text — trimmed, compared after Unicode lowercasing, an empty
//! search matching every name — **and** its kind is among the toggled
//! kinds (pixel, adjustment, group; none toggled means every kind). The
//! panel shows the matching layers **together with their ancestor
//! groups**, so the tree context stays: ancestors are forced open while
//! the filter is active (their own collapsed state is remembered and comes
//! back when the filter clears — [`crate::layers_panel::LayersView`]) and
//! are drawn like any other row (no existing token dims a row without also
//! disabling it). A group that matches shows only its matching
//! descendants; one with none shows as a leaf. Everything else is hidden.
//! An empty result shows "No matching layers".
//!
//! **Matching limits.** The text is trimmed and compared after
//! `str::to_lowercase` only: no case folding and no Unicode normalisation,
//! so "ß" does not match "ss" and a decomposed (NFD, common from macOS)
//! name does not match composed (NFC) input. Whitespace around the text
//! never changes the rows, so it does not rebuild the list.
//!
//! **Not document state.** The filter is view state of one document
//! session: no undo, no History row, never in the `.aur`, never dirty. The
//! *active layer* may be hidden by it; the controls strip keeps editing it
//! and its row highlight shows whenever its row is shown.

use std::collections::HashSet;

use aurora_doc::{LayerId, LayerKind, LayerTree};
use aurora_theme::Scales;
use aurora_widgets::icons::IconKind;
use aurora_widgets::widgets::{self, IconSize, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};

/// The accessible name of the search field.
pub const SEARCH_LABEL: &str = "Search layers";
/// The field's placeholder (0.199.0, judge I5): shorter than its accessible
/// name so a screen reader does not read the same words twice.
pub const SEARCH_PLACEHOLDER: &str = "Search";
/// The text shown when nothing matches.
pub const NO_MATCH_TEXT: &str = "No matching layers";

/// One of the three kind toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KindToggle {
    Pixel,
    Adjustment,
    Group,
}

impl KindToggle {
    /// The toggles in row order.
    pub const ALL: [KindToggle; 3] = [KindToggle::Pixel, KindToggle::Adjustment, KindToggle::Group];

    /// The button's accessible name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            KindToggle::Pixel => "Show pixel layers",
            KindToggle::Adjustment => "Show adjustment layers",
            KindToggle::Group => "Show groups",
        }
    }

    /// The button's description.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            KindToggle::Pixel => "Filter the list to pixel layers",
            KindToggle::Adjustment => "Filter the list to adjustment layers",
            KindToggle::Group => "Filter the list to groups",
        }
    }

    /// The icon.
    #[must_use]
    pub const fn icon(self) -> IconKind {
        match self {
            KindToggle::Pixel => IconKind::Image,
            KindToggle::Adjustment => IconKind::Adjustment,
            KindToggle::Group => IconKind::FolderClosed,
        }
    }
}

/// The filter of one document session's Layers panel. `Default` is "no
/// filter": empty text, no kind toggled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayerFilter {
    /// The search text as typed (trimmed when matched).
    pub text: String,
    pub pixel: bool,
    pub adjustment: bool,
    pub group: bool,
}

impl LayerFilter {
    /// Whether `toggle` is on.
    #[must_use]
    pub fn kind_on(&self, toggle: KindToggle) -> bool {
        match toggle {
            KindToggle::Pixel => self.pixel,
            KindToggle::Adjustment => self.adjustment,
            KindToggle::Group => self.group,
        }
    }

    /// Sets `toggle`.
    pub fn set_kind(&mut self, toggle: KindToggle, on: bool) {
        match toggle {
            KindToggle::Pixel => self.pixel = on,
            KindToggle::Adjustment => self.adjustment = on,
            KindToggle::Group => self.group = on,
        }
    }

    /// Whether the filter hides anything at all: non-blank text or a kind
    /// toggled.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.text.trim().is_empty() || self.pixel || self.adjustment || self.group
    }

    fn kind_allowed(&self, kind: &LayerKind) -> bool {
        if !(self.pixel || self.adjustment || self.group) {
            return true;
        }
        match kind {
            LayerKind::Pixel { .. } => self.pixel,
            LayerKind::Adjustment(_) => self.adjustment,
            LayerKind::Group { .. } => self.group,
        }
    }

    /// Whether `id` matches by itself (name and kind), ancestors aside.
    #[must_use]
    pub fn matches(&self, layers: &LayerTree, id: LayerId) -> bool {
        let needle = self.text.trim().to_lowercase();
        self.matches_with(layers, id, &needle)
    }

    fn matches_with(&self, layers: &LayerTree, id: LayerId, needle: &str) -> bool {
        let Some(kind) = layers.kind(id) else {
            return false;
        };
        if !self.kind_allowed(kind) {
            return false;
        }
        needle.is_empty()
            || crate::layers_panel::display_name_of(layers, id)
                .to_lowercase()
                .contains(needle)
    }
}

/// The layers the panel shows under `filter`: every match plus its
/// ancestors; `None` when the filter is not active (everything shows).
#[must_use]
pub fn shown_layers(layers: &LayerTree, filter: &LayerFilter) -> Option<HashSet<LayerId>> {
    if !filter.is_active() {
        return None;
    }
    let needle = filter.text.trim().to_lowercase();
    let mut shown = HashSet::new();
    let mut pending: Vec<LayerId> = layers.roots().to_vec();
    while let Some(id) = pending.pop() {
        pending.extend(layers.children(id).unwrap_or_default().iter().copied());
        if !filter.matches_with(layers, id, &needle) {
            continue;
        }
        let mut at = Some(id);
        while let Some(current) = at {
            if !shown.insert(current) {
                break;
            }
            at = layers.parent(current);
        }
    }
    Some(shown)
}

/// The filter row's widgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerFilterRow {
    pub row: WidgetId,
    pub field: WidgetId,
    pub icon: WidgetId,
    pub pixel: WidgetId,
    pub adjustment: WidgetId,
    pub group: WidgetId,
}

impl LayerFilterRow {
    /// The button for `toggle`.
    #[must_use]
    pub fn button(&self, toggle: KindToggle) -> WidgetId {
        match toggle {
            KindToggle::Pixel => self.pixel,
            KindToggle::Adjustment => self.adjustment,
            KindToggle::Group => self.group,
        }
    }

    /// The toggle `widget` is, if it is one.
    #[must_use]
    pub fn toggle_for(&self, widget: WidgetId) -> Option<KindToggle> {
        KindToggle::ALL
            .into_iter()
            .find(|&toggle| self.button(toggle) == widget)
    }
}

/// The icon size of the kind toggles and the search glyph.
pub(crate) const FILTER_ICON_SIZE: IconSize = IconSize::Small;

/// The row's height: its toggles, or one row if they are shorter.
pub(crate) fn filter_row_height(scales: &Scales) -> f32 {
    widgets::icon_button_side(scales, FILTER_ICON_SIZE).max(widgets::row_height(scales))
}

/// Builds the row under `root` (the controls strip) as its **first**
/// child.
///
/// # Errors
///
/// Whatever the widget inserts return.
pub(crate) fn insert_filter_row(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    scales: &Scales,
    row_style: taffy::Style,
) -> Result<LayerFilterRow, WidgetError> {
    let mut node = accesskit::Node::new(accesskit::Role::Group);
    node.set_label("Filter layers".to_owned());
    let row = tree.insert(root, row_style, node, WidgetKind::Container)?;
    tree.move_child(row, root, 0)?;
    let icon = widgets::insert_icon(
        tree,
        row,
        scales,
        IconKind::Search,
        FILTER_ICON_SIZE,
        widgets::IconTone::Secondary,
        widgets::IconAccess::Decorative,
    )?;
    let field = widgets::insert_text_field(tree, row, scales, SEARCH_LABEL, "")?;
    widgets::set_text_field_hints(tree, field, Some(SEARCH_PLACEHOLDER), None)?;
    let mut field_style = tree.style(field).cloned().unwrap_or_default();
    field_style.min_size.width = taffy::style_helpers::zero();
    tree.set_style(field, field_style)?;
    let mut buttons = [row; 3];
    for (slot, toggle) in buttons.iter_mut().zip(KindToggle::ALL) {
        let button = widgets::insert_icon_toggle_button(
            tree,
            row,
            scales,
            toggle.label(),
            toggle.icon(),
            FILTER_ICON_SIZE,
            false,
        )?;
        widgets::set_button_description(tree, button, Some(toggle.description()))?;
        *slot = button;
    }
    let [pixel, adjustment, group] = buttons;
    Ok(LayerFilterRow {
        row,
        field,
        icon,
        pixel,
        adjustment,
        group,
    })
}

/// The filter the row's widgets currently show.
#[must_use]
pub fn read_layer_filter(tree: &WidgetTree<WidgetKind>, row: &LayerFilterRow) -> LayerFilter {
    let on = |id| matches!(tree.payload(id), Some(WidgetKind::Button(state)) if state.toggled == Some(true));
    LayerFilter {
        text: match tree.payload(row.field) {
            Some(WidgetKind::TextField(state)) => state.content.clone(),
            _ => String::new(),
        },
        pixel: on(row.pixel),
        adjustment: on(row.adjustment),
        group: on(row.group),
    }
}

/// Makes the row's widgets show `filter` (only what differs is touched).
/// Returns whether anything changed.
///
/// # Errors
///
/// Whatever the widget setters return.
pub fn write_layer_filter(
    tree: &mut WidgetTree<WidgetKind>,
    row: &LayerFilterRow,
    filter: &LayerFilter,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    if read_layer_filter(tree, row).text != filter.text {
        widgets::with_text_field_mut(tree, row.field, |state| {
            state.content.clone_from(&filter.text);
            state.cursor = state.content.len();
            state.selection_anchor = None;
            state.composition = None;
        })?;
        tree.mark_dirty(row.field)?;
        changed = true;
    }
    for toggle in KindToggle::ALL {
        changed |= widgets::set_button_toggled(tree, row.button(toggle), filter.kind_on(toggle))?;
    }
    Ok(changed)
}

/// Clears the search text as an edit of the field (so its own undo can
/// bring it back). Returns whether there was text to clear.
///
/// # Errors
///
/// Whatever [`widgets::with_text_field_mut`] returns.
pub fn clear_layer_filter_text(
    tree: &mut WidgetTree<WidgetKind>,
    row: &LayerFilterRow,
) -> Result<bool, WidgetError> {
    widgets::with_text_field_mut(tree, row.field, |state| {
        if state.content.is_empty() {
            return false;
        }
        state.select_all();
        state.backspace();
        true
    })
}

/// Sets the field's accessible description to "12 of 277 layers shown"
/// (or none when no filter is active). A description only, so nothing is
/// announced. Returns whether it changed.
///
/// # Errors
///
/// As [`widgets::set_text_field_hints`].
pub fn set_layer_filter_summary(
    tree: &mut WidgetTree<WidgetKind>,
    row: &LayerFilterRow,
    shown: usize,
    total: usize,
    active: bool,
) -> Result<bool, WidgetError> {
    let text = active.then(|| format!("{shown} of {total} layers shown"));
    widgets::set_text_field_hints(tree, row.field, Some(SEARCH_PLACEHOLDER), text.as_deref())
}

/// Whether `id` is the filter field of `row`.
#[must_use]
pub fn is_layer_filter_field(row: Option<&LayerFilterRow>, id: WidgetId) -> bool {
    row.is_some_and(|row| row.field == id)
}
