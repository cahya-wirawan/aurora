//! The Channels panel (0.232.0): Photoshop's Channels panel, version one.
//!
//! **What it shows.** Four *composite* rows, `RGB`, `Red`, `Green` and
//! `Blue`, each a flat tree row with an eye; under a separator one row per
//! *saved selection* (the document's named selections, which this panel
//! presents as its alpha channels); with none saved, a single
//! non-interactive "No saved selections" label. A toolbar at the panel's
//! foot has two icon buttons: New channel from selection (the Save
//! Selection prompt) and Delete (the selected saved selection).
//!
//! **The eyes are a view setting.** [`ChannelVisibility`] says which of
//! the three colour channels the canvas *displays*; it is not document
//! state (no undo step, never dirty, not saved), and a hidden channel is
//! only zeroed in the displayed picture (`aurora-gpu`'s canvas shader
//! multiplies by [`ChannelVisibility::gain`]). The `RGB` row reads on only
//! when all three are on, and toggling it shows or hides all three.
//!
//! **Selection and activation.** A press on a row selects it (a saved row
//! then enables Delete); a press on an eye toggles it; a saved row *loads
//! as the selection* on a double click, `Enter`, or an assistive
//! technology's `Click` (the default action of the row). That is
//! `aurora-app`'s call; this module only builds the rows and answers which
//! row or part a widget is.
//!
//! **Not here (v1).** No thumbnails, no per-channel editing or painting,
//! no single-channel greyscale display, no spot channels, no Ctrl-click to
//! load, and no Paths tab (a path model needs the Pen tool first).
//!
//! The rows are *rebuilt* when the saved-selection names change
//! ([`sync_channels_panel`]) and only *re-iconed in place* when an eye
//! changes, so a row's id and focus survive a toggle.

use accesskit::{Node, Orientation, Role};
use aurora_theme::Scales;
use aurora_widgets::icons::IconKind;
use aurora_widgets::widgets::{
    self, IconAccess, IconSize, IconTone, WidgetKind, icon_state, insert_icon, insert_tree_item,
    insert_tree_view, set_icon_access, set_icon_kind, set_icon_tone, set_tree_item_description,
    set_tree_item_flat, set_tree_item_selected,
};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::{TaffyZero as _, length, percent};
use taffy::{
    AlignItems, Dimension, FlexDirection, JustifyContent, Rect as LayoutRect, Size, Style,
};

use crate::layers_panel::{eye_access, eye_column, eye_kind, icon_side, place_icon};
use crate::panel::{PanelHandle, clear_panel_body};

/// The toolbar's accessible name.
pub const CHANNELS_TOOLBAR_LABEL: &str = "Channel actions";

/// The empty state's text.
pub const NO_SAVED_SELECTIONS_TEXT: &str = "No saved selections";

/// The icon size the toolbar's buttons use (the Layers toolbar's).
pub const CHANNELS_TOOLBAR_ICON_SIZE: IconSize = IconSize::Large;

/// One of the four composite rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompositeChannel {
    Rgb,
    Red,
    Green,
    Blue,
}

impl CompositeChannel {
    /// Every composite row, in panel order.
    pub const ALL: [Self; 4] = [Self::Rgb, Self::Red, Self::Green, Self::Blue];

    /// The row's name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rgb => "RGB",
            Self::Red => "Red",
            Self::Green => "Green",
            Self::Blue => "Blue",
        }
    }
}

/// Which colour channels the canvas displays — a per-document *view*
/// setting (see the module docs). Defaults to all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelVisibility {
    pub red: bool,
    pub green: bool,
    pub blue: bool,
}

impl Default for ChannelVisibility {
    fn default() -> Self {
        Self::ALL_ON
    }
}

impl ChannelVisibility {
    /// Every channel shown — the state the canvas draws unchanged in.
    pub const ALL_ON: Self = Self {
        red: true,
        green: true,
        blue: true,
    };

    /// Whether all three channels are shown.
    #[must_use]
    pub const fn all_on(self) -> bool {
        self.red && self.green && self.blue
    }

    /// Whether `channel`'s eye reads on (`RGB`: only when all three are).
    #[must_use]
    pub const fn is_on(self, channel: CompositeChannel) -> bool {
        match channel {
            CompositeChannel::Rgb => self.all_on(),
            CompositeChannel::Red => self.red,
            CompositeChannel::Green => self.green,
            CompositeChannel::Blue => self.blue,
        }
    }

    /// `channel`'s eye pressed: a colour channel flips; `RGB` hides all
    /// three when all are on and shows all three otherwise.
    #[must_use]
    pub const fn toggled(self, channel: CompositeChannel) -> Self {
        match channel {
            CompositeChannel::Rgb => {
                let on = !self.all_on();
                Self {
                    red: on,
                    green: on,
                    blue: on,
                }
            }
            CompositeChannel::Red => Self {
                red: !self.red,
                ..self
            },
            CompositeChannel::Green => Self {
                green: !self.green,
                ..self
            },
            CompositeChannel::Blue => Self {
                blue: !self.blue,
                ..self
            },
        }
    }

    /// The per-channel multiplier the canvas applies to what it displays:
    /// `1.0` for a shown channel, `0.0` for a hidden one.
    #[must_use]
    pub const fn gain(self) -> [f32; 3] {
        [
            if self.red { 1.0 } else { 0.0 },
            if self.green { 1.0 } else { 0.0 },
            if self.blue { 1.0 } else { 0.0 },
        ]
    }
}

/// What a row stands for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChannelRow {
    Composite(CompositeChannel),
    /// A saved selection, by name.
    Saved(String),
}

/// Which part of a row a widget is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRowPart {
    Row,
    Eye,
}

/// What a toolbar button asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelsAction {
    /// Save the active selection under a new name (the Save Selection
    /// prompt).
    NewFromSelection,
    /// Delete the selected saved selection.
    Delete,
}

impl ChannelsAction {
    /// Every action, in toolbar order.
    pub const ALL: [Self; 2] = [Self::NewFromSelection, Self::Delete];

    /// The button's accessible name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::NewFromSelection => "New channel from selection",
            Self::Delete => "Delete channel",
        }
    }

    /// The icon on the button. There is no dedicated "channel" glyph yet,
    /// so the New Layer and Delete icons are reused.
    #[must_use]
    pub const fn icon(self) -> IconKind {
        match self {
            Self::NewFromSelection => IconKind::NewLayer,
            Self::Delete => IconKind::Delete,
        }
    }

    /// The tooltip text and accessible description.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::NewFromSelection => "Save the current selection as a named channel",
            Self::Delete => "Delete the selected saved selection",
        }
    }
}

/// Why Delete is unavailable (its description while disabled).
pub const DELETE_UNAVAILABLE: &str = "Select a saved selection to delete it";

/// The toolbar and its buttons, in [`ChannelsAction::ALL`] order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelsToolbar {
    pub root: WidgetId,
    pub buttons: [(ChannelsAction, WidgetId); ChannelsAction::ALL.len()],
}

impl ChannelsToolbar {
    /// The action button `id` stands for.
    #[must_use]
    pub fn action_for(&self, id: WidgetId) -> Option<ChannelsAction> {
        self.buttons
            .iter()
            .find(|(_, button)| *button == id)
            .map(|(action, _)| *action)
    }

    /// `action`'s button.
    #[must_use]
    pub fn button_for(&self, action: ChannelsAction) -> Option<WidgetId> {
        self.buttons
            .iter()
            .find(|(candidate, _)| *candidate == action)
            .map(|(_, button)| *button)
    }
}

/// The panel's live state: the toolbar, the rows as built (in screen
/// order), what they were built for and the selected row.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelsPanel {
    pub toolbar: ChannelsToolbar,
    /// Every interactive row, top to bottom.
    pub rows: Vec<(WidgetId, ChannelRow)>,
    /// The saved-selection names the rows were built for.
    pub names: Vec<String>,
    /// The eye states the composite rows show.
    pub visibility: ChannelVisibility,
    /// The selected row; a saved one that no longer exists falls back to
    /// `RGB` at the next [`populate_channels_panel`].
    pub selected: ChannelRow,
}

impl ChannelsPanel {
    /// The id of `row`'s widget, if shown.
    #[must_use]
    pub fn id_of(&self, row: &ChannelRow) -> Option<WidgetId> {
        self.rows.iter().find(|(_, r)| r == row).map(|(id, _)| *id)
    }

    /// What `id` stands for, when it is one of the rows.
    #[must_use]
    pub fn row_of(&self, id: WidgetId) -> Option<&ChannelRow> {
        self.rows.iter().find(|(r, _)| *r == id).map(|(_, r)| r)
    }

    /// The saved selection currently selected, if one is.
    #[must_use]
    pub fn selected_saved(&self) -> Option<&str> {
        match &self.selected {
            ChannelRow::Saved(name) => Some(name),
            ChannelRow::Composite(_) => None,
        }
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

    /// Which row (and part of it) `hit` is: `hit` itself or an ancestor
    /// that is a row; an eye icon is told by being the named button.
    #[must_use]
    pub fn row_part(
        &self,
        tree: &WidgetTree<WidgetKind>,
        hit: WidgetId,
    ) -> Option<(WidgetId, ChannelRowPart)> {
        let mut part = ChannelRowPart::Row;
        let mut current = Some(hit);
        while let Some(id) = current {
            if self.rows.iter().any(|(row, _)| *row == id) {
                return Some((id, part));
            }
            if let Some(WidgetKind::Icon(state)) = tree.payload(id)
                && matches!(state.access, IconAccess::Button { .. })
            {
                part = ChannelRowPart::Eye;
            }
            current = tree.parent(id);
        }
        None
    }
}

#[allow(clippy::cast_precision_loss)]
fn px(token: u32) -> f32 {
    token as f32
}

/// The toolbar row's height: a button plus `spacing.xxs` above and below.
#[must_use]
pub fn channels_toolbar_height(scales: &Scales) -> f32 {
    widgets::icon_button_side(scales, CHANNELS_TOOLBAR_ICON_SIZE) + 2.0 * px(scales.spacing.xxs)
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
            height: length(channels_toolbar_height(scales)),
        },
        min_size: Size {
            width: Dimension::ZERO,
            height: length(channels_toolbar_height(scales)),
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

/// Adds the toolbar to `panel` (the Channels panel) as the last child of
/// its root and refreshes the panel's height floor. Delete starts
/// disabled.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `panel.root` doesn't exist; a partly
/// built toolbar is removed again.
pub fn insert_channels_toolbar(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
) -> Result<ChannelsToolbar, WidgetError> {
    let mut node = Node::new(Role::Toolbar);
    node.set_label(CHANNELS_TOOLBAR_LABEL);
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
) -> Result<ChannelsToolbar, WidgetError> {
    let mut buttons = [(ChannelsAction::NewFromSelection, root); ChannelsAction::ALL.len()];
    for (slot, action) in buttons.iter_mut().zip(ChannelsAction::ALL) {
        let id = widgets::insert_icon_button(
            tree,
            root,
            scales,
            action.name(),
            action.icon(),
            CHANNELS_TOOLBAR_ICON_SIZE,
        )?;
        let text = if action == ChannelsAction::Delete {
            widgets::set_button_disabled(tree, id, true)?;
            DELETE_UNAVAILABLE
        } else {
            action.description()
        };
        widgets::set_button_description(tree, id, Some(text))?;
        *slot = (action, id);
    }
    Ok(ChannelsToolbar { root, buttons })
}

/// Mirrors whether a saved selection is selected into Delete (enabled
/// exactly then, its description saying why not otherwise). Returns
/// whether anything changed.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`] for a
/// missing or non-button widget.
pub fn sync_channels_toolbar(
    tree: &mut WidgetTree<WidgetKind>,
    toolbar: &ChannelsToolbar,
    delete_enabled: bool,
) -> Result<bool, WidgetError> {
    let Some(button) = toolbar.button_for(ChannelsAction::Delete) else {
        return Ok(false);
    };
    let was_disabled = matches!(
        tree.payload(button),
        Some(WidgetKind::Button(state)) if state.disabled
    );
    let mut changed = false;
    if was_disabled == delete_enabled {
        widgets::set_button_disabled(tree, button, !delete_enabled)?;
        changed = true;
    }
    let text = if delete_enabled {
        ChannelsAction::Delete.description()
    } else {
        DELETE_UNAVAILABLE
    };
    changed |= widgets::set_button_description(tree, button, Some(text))?;
    Ok(changed)
}

/// Points the hover tint at `hovered` (a button of this toolbar); `None`
/// clears it. Returns whether anything changed.
///
/// # Errors
///
/// As [`sync_channels_toolbar`].
pub fn set_channels_toolbar_hover(
    tree: &mut WidgetTree<WidgetKind>,
    toolbar: &ChannelsToolbar,
    hovered: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    for (_, id) in toolbar.buttons {
        changed |= widgets::set_button_hovered(tree, id, hovered == Some(id))?;
    }
    Ok(changed)
}

/// Whether `id` is one of the Channel actions toolbar's buttons (read off
/// the tree: an icon button whose parent is the labelled toolbar).
#[must_use]
pub fn is_channels_toolbar_button(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    matches!(tree.payload(id), Some(WidgetKind::Button(state)) if state.icon.is_some())
        && tree
            .parent(id)
            .and_then(|parent| tree.accessibility(parent))
            .is_some_and(|node| {
                node.role() == Role::Toolbar && node.label() == Some(CHANNELS_TOOLBAR_LABEL)
            })
}

/// Builds the panel's content state with an empty row list; the caller
/// then runs [`populate_channels_panel`].
#[must_use]
pub fn new_channels_panel(toolbar: ChannelsToolbar) -> ChannelsPanel {
    ChannelsPanel {
        toolbar,
        rows: Vec::new(),
        names: Vec::new(),
        visibility: ChannelVisibility::ALL_ON,
        selected: ChannelRow::Composite(CompositeChannel::Rgb),
    }
}

fn eye_name(channel: CompositeChannel) -> String {
    format!("{} channel", channel.label())
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

fn insert_row(
    tree: &mut WidgetTree<WidgetKind>,
    container: WidgetId,
    scales: &Scales,
    label: &str,
    eye: Option<(bool, &str)>,
) -> Result<WidgetId, WidgetError> {
    let row = insert_tree_item(tree, container, scales, label, false)?;
    let inset = eye_column(scales) + px(scales.spacing.xs);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    set_tree_item_flat(tree, row, inset.round() as u32)?;
    if let Some((visible, name)) = eye {
        let side = icon_side(scales);
        let line = widgets::row_height(scales);
        let top = ((line - side) / 2.0).floor().max(0.0);
        let icon = insert_icon(
            tree,
            row,
            scales,
            eye_kind(visible),
            IconSize::Small,
            IconTone::Default,
            eye_access(name, visible),
        )?;
        place_icon(tree, icon, (eye_column(scales) - side) / 2.0, top)?;
    }
    Ok(row)
}

/// Rebuilds the panel's rows for `visibility` and the saved-selection
/// `names` (sorted by the caller): the four composite rows, a separator,
/// then a row per name or the empty-state label. Keeps `state.selected`
/// when it still exists, else selects `RGB`; mirrors the selection into
/// Delete. Safe to repeat.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if the panel's body is missing.
pub fn populate_channels_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
    state: &mut ChannelsPanel,
    visibility: ChannelVisibility,
    names: &[String],
) -> Result<(), WidgetError> {
    clear_panel_body(tree, panel.body)?;
    let composite = insert_tree_view(tree, panel.body, None)?;
    tree.set_style(composite, tree_container_style())?;
    let mut rows = Vec::new();
    for channel in CompositeChannel::ALL {
        let name = eye_name(channel);
        let row = insert_row(
            tree,
            composite,
            scales,
            channel.label(),
            Some((visibility.is_on(channel), &name)),
        )?;
        rows.push((row, ChannelRow::Composite(channel)));
    }
    widgets::insert_separator(tree, panel.body, scales)?;
    if names.is_empty() {
        let label = widgets::insert_label(tree, panel.body, scales, NO_SAVED_SELECTIONS_TEXT)?;
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
        let saved = insert_tree_view(tree, panel.body, None)?;
        tree.set_style(saved, tree_container_style())?;
        for name in names {
            let row = insert_row(tree, saved, scales, name, None)?;
            set_tree_item_description(
                tree,
                row,
                Some("Saved selection. Press Enter to load it as the selection."),
            )?;
            rows.push((row, ChannelRow::Saved(name.clone())));
        }
    }
    state.rows = rows;
    state.names = names.to_vec();
    state.visibility = visibility;
    let keep = state.rows.iter().any(|(_, row)| *row == state.selected);
    if !keep {
        state.selected = ChannelRow::Composite(CompositeChannel::Rgb);
    }
    let selected = state.selected.clone();
    select_channel_row(tree, state, &selected)
}

/// Makes `row` the selected one: highlights it, retones the icons of the
/// rows that changed and mirrors the selection into Delete. A `row` that
/// is not shown selects `RGB`.
///
/// # Errors
///
/// As the widget setters it calls.
pub fn select_channel_row(
    tree: &mut WidgetTree<WidgetKind>,
    state: &mut ChannelsPanel,
    row: &ChannelRow,
) -> Result<(), WidgetError> {
    let wanted = if state.id_of(row).is_some() {
        row.clone()
    } else {
        ChannelRow::Composite(CompositeChannel::Rgb)
    };
    for (id, kind) in state.rows.clone() {
        let selected = kind == wanted;
        set_tree_item_selected(tree, id, selected)?;
        retone_row(tree, id, selected)?;
    }
    state.selected = wanted;
    sync_channels_toolbar(tree, &state.toolbar, state.selected_saved().is_some())?;
    Ok(())
}

fn retone_row(
    tree: &mut WidgetTree<WidgetKind>,
    row: WidgetId,
    selected: bool,
) -> Result<(), WidgetError> {
    for child in tree.children(row).unwrap_or_default().to_vec() {
        let Some(WidgetKind::Icon(icon)) = tree.payload(child) else {
            continue;
        };
        let off = icon.kind == IconKind::EyeOff;
        let tone = if selected {
            IconTone::OnAccent
        } else if off {
            IconTone::Secondary
        } else {
            IconTone::Default
        };
        set_icon_tone(tree, child, tone)?;
    }
    Ok(())
}

/// Brings the composite rows' eyes in line with `visibility` in place
/// (picture, name, `toggled`, tone), never inserting or removing a
/// widget. Returns whether anything changed.
///
/// # Errors
///
/// As the widget setters it calls.
pub fn sync_channel_eyes(
    tree: &mut WidgetTree<WidgetKind>,
    state: &mut ChannelsPanel,
    visibility: ChannelVisibility,
) -> Result<bool, WidgetError> {
    let mut changed = state.visibility != visibility;
    for (row, kind) in state.rows.clone() {
        let ChannelRow::Composite(channel) = kind else {
            continue;
        };
        let selected = state.selected == ChannelRow::Composite(channel);
        for child in tree.children(row).unwrap_or_default().to_vec() {
            let is_eye = matches!(
                tree.payload(child),
                Some(WidgetKind::Icon(icon)) if matches!(icon.access, IconAccess::Button { .. })
            );
            if !is_eye {
                continue;
            }
            let on = visibility.is_on(channel);
            changed |= set_icon_kind(tree, child, eye_kind(on))?;
            changed |= set_icon_access(tree, child, eye_access(&eye_name(channel), on))?;
            let tone = if selected {
                IconTone::OnAccent
            } else if on {
                IconTone::Default
            } else {
                IconTone::Secondary
            };
            changed |= set_icon_tone(tree, child, tone)?;
        }
    }
    state.visibility = visibility;
    Ok(changed)
}

/// Keeps the panel current: rebuilds the rows when the saved-selection
/// `names` changed since they were built, else only re-icons the eyes when
/// `visibility` changed. Cheap when nothing changed (two comparisons), so
/// a caller runs it every event-loop turn. Returns whether anything
/// changed (so a caller can re-lay-out).
///
/// # Errors
///
/// As [`populate_channels_panel`] / [`sync_channel_eyes`].
pub fn sync_channels_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
    state: &mut ChannelsPanel,
    visibility: ChannelVisibility,
    names: &[String],
) -> Result<bool, WidgetError> {
    // A closed panel's body is emptied (`close_panel`), which leaves the
    // remembered rows dead: they are rebuilt like a name change.
    let dead = state.rows.first().is_none_or(|(id, _)| !tree.contains(*id));
    if dead || state.names != names {
        populate_channels_panel(tree, panel, scales, state, visibility, names)?;
        return Ok(true);
    }
    if state.visibility != visibility {
        return sync_channel_eyes(tree, state, visibility);
    }
    Ok(false)
}

/// Whether the eye icon `id` is a named toggle (test support and routing
/// helper).
#[must_use]
pub fn eye_is_on(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Option<bool> {
    let state = icon_state(tree, id).ok()?;
    match state.access {
        IconAccess::Button { toggled, .. } => toggled,
        IconAccess::Decorative | IconAccess::Named(_) => None,
    }
}

#[cfg(test)]
#[path = "channels_panel_tests.rs"]
mod tests;
