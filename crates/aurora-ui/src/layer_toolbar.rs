//! The Layers panel's bottom toolbar (0.196.0, round L3 of the
//! Photoshop-style Layers panel): seven flat icon buttons, right-aligned
//! under the list — Link, Fx, Mask, Adjustment, Group, New Layer, Delete,
//! in that order, as in the reference.
//!
//! **Where it lives.** Like the controls strip ([`crate::layer_controls`])
//! it is a child of the panel's own *root*, appended after the list's
//! viewport, never inside `panel.body`: [`crate::populate_layers_panel`]
//! clears the body on every call, so a toolbar in it would be rebuilt (and
//! lose focus) on every structural change. Built once by
//! [`crate::build_workspace`]; it survives every repopulation, and
//! [`crate::set_panel_collapsed`] hides it with the body. Its minimum
//! height joins the panel's floor ([`crate::panel`]'s `refresh_panel_floor`),
//! so a short rail squeezes the list, never the toolbar.
//!
//! **Buttons.** Each is a real icon `Button`
//! ([`aurora_widgets::widgets::insert_icon_button`]): its own `Tab` stop,
//! `Space`/`Enter`, a click, an assistive technology's `Click`, hover and
//! pressed tints and the focus ring from tokens. The toolbar owns no
//! document state: `aurora-app` maps an activated button to its command
//! ([`LayerToolbar::action_for`]) and mirrors what is allowed back with
//! [`sync_layer_toolbar`].
//!
//! **Placeholders.** Link, Fx and Mask are *disabled* for good: none of
//! those features exists in the UI yet (design owner, 2026-10-11). They
//! can never run anything — a disabled button has no `Click` action and
//! cannot be pressed — and each says why in its description
//! ([`LayerAction::description`]), the text a tooltip shows and a screen
//! reader reads. **Tooltips:** the toolkit's `Tooltip` hangs below its
//! owner at the owner's width, which cannot hold these sentences at the
//! bottom edge of the rail, so the description is the carrier for now.

use accesskit::{Node, Orientation, Role};
use aurora_theme::Scales;
use aurora_widgets::icons::IconKind;
use aurora_widgets::widgets::{self, IconSize, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::{TaffyZero as _, length};
use taffy::{
    AlignItems, Dimension, FlexDirection, JustifyContent, Rect as LayoutRect, Size, Style,
};

use crate::panel::PanelHandle;

/// The toolbar's accessible name.
pub const LAYER_TOOLBAR_LABEL: &str = "Layer actions";

/// The icon size the toolbar's buttons use.
pub const LAYER_TOOLBAR_ICON_SIZE: IconSize = IconSize::Large;

/// What a toolbar button asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayerAction {
    /// Link layers — a disabled placeholder.
    Link,
    /// Layer effects — a disabled placeholder until the effects UI exists.
    Fx,
    /// Add a layer mask — a disabled placeholder.
    Mask,
    /// New adjustment layer (a Curves layer).
    Adjustment,
    /// New group.
    Group,
    /// New layer.
    NewLayer,
    /// Delete the active layer or group.
    Delete,
}

impl LayerAction {
    /// Every action, in toolbar order.
    pub const ALL: [LayerAction; 7] = [
        LayerAction::Link,
        LayerAction::Fx,
        LayerAction::Mask,
        LayerAction::Adjustment,
        LayerAction::Group,
        LayerAction::NewLayer,
        LayerAction::Delete,
    ];

    /// The button's accessible name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            LayerAction::Link => "Link layers",
            LayerAction::Fx => "Layer effects",
            LayerAction::Mask => "Add layer mask",
            LayerAction::Adjustment => "New adjustment layer",
            LayerAction::Group => "New group",
            LayerAction::NewLayer => "New layer",
            LayerAction::Delete => "Delete layer",
        }
    }

    /// The icon on the button.
    #[must_use]
    pub const fn icon(self) -> IconKind {
        match self {
            LayerAction::Link => IconKind::Link,
            LayerAction::Fx => IconKind::Fx,
            LayerAction::Mask => IconKind::Mask,
            LayerAction::Adjustment => IconKind::Adjustment,
            LayerAction::Group => IconKind::Group,
            LayerAction::NewLayer => IconKind::NewLayer,
            LayerAction::Delete => IconKind::Delete,
        }
    }

    /// Whether the button is a permanent disabled placeholder.
    #[must_use]
    pub const fn is_placeholder(self) -> bool {
        matches!(
            self,
            LayerAction::Link | LayerAction::Fx | LayerAction::Mask
        )
    }

    /// The tooltip text and accessible description: what the button does,
    /// or — for a placeholder — why it cannot yet.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            LayerAction::Link => "Layers can't be linked from here yet",
            LayerAction::Fx => "Layer effects can't be added from here yet",
            LayerAction::Mask => "Layer masks can't be added from here yet",
            LayerAction::Adjustment => "Add a Curves adjustment layer above the selected layer",
            LayerAction::Group => "Add an empty group above the selected layer",
            LayerAction::NewLayer => "Add an empty layer above the selected layer",
            LayerAction::Delete => "Delete the selected layer or group",
        }
    }
}

/// Why Delete is unavailable (its description while disabled).
pub const DELETE_UNAVAILABLE: &str =
    "Can't delete: a document needs at least one pixel layer, and one must stay selected";

/// The toolbar and its buttons, one per [`LayerAction::ALL`] entry, in
/// that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerToolbar {
    /// The `Role::Toolbar` row — a child of the Layers panel's root.
    pub root: WidgetId,
    /// `(action, its button)`, in [`LayerAction::ALL`] order.
    pub buttons: [(LayerAction, WidgetId); LayerAction::ALL.len()],
}

impl LayerToolbar {
    /// The action button `id` stands for, if it is one of this toolbar's.
    #[must_use]
    pub fn action_for(&self, id: WidgetId) -> Option<LayerAction> {
        self.buttons
            .iter()
            .find(|(_, button)| *button == id)
            .map(|(action, _)| *action)
    }

    /// `action`'s button.
    #[must_use]
    pub fn button_for(&self, action: LayerAction) -> Option<WidgetId> {
        self.buttons
            .iter()
            .find(|(candidate, _)| *candidate == action)
            .map(|(_, button)| *button)
    }
}

/// The toolbar row's height: a button plus `spacing.xxs` above and below.
#[must_use]
pub fn layer_toolbar_height(scales: &Scales) -> f32 {
    widgets::icon_button_side(scales, LAYER_TOOLBAR_ICON_SIZE) + 2.0 * px(scales.spacing.xxs)
}

/// [`layer_toolbar_height`] in whole logical px, for tests that add it to
/// whole-row arithmetic.
#[cfg(test)]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn toolbar_px(scales: &Scales) -> u32 {
    layer_toolbar_height(scales) as u32
}

#[allow(clippy::cast_precision_loss)]
fn px(token: u32) -> f32 {
    token as f32
}

fn toolbar_style(scales: &Scales) -> Style {
    let pad = length(px(scales.spacing.xxs));
    Style {
        flex_direction: FlexDirection::Row,
        justify_content: Some(JustifyContent::FLEX_END),
        align_items: Some(AlignItems::CENTER),
        // Never shrunk: the list's viewport takes a crowded rail's
        // squeeze, and the row's own height is part of the panel floor.
        flex_shrink: 0.0,
        size: Size {
            width: Dimension::auto(),
            height: length(layer_toolbar_height(scales)),
        },
        min_size: Size {
            width: Dimension::ZERO,
            height: length(layer_toolbar_height(scales)),
        },
        // No gap: each button already carries `spacing.xxs` of room round
        // its icon, and seven 32 px buttons plus gaps would not fit the
        // default 250 px rail inside the row's padding.
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

/// Adds the toolbar to `panel` (the Layers panel) as the last child of its
/// root and refreshes the panel's height floor. Every button starts with
/// its description; Link, Fx and Mask start (and stay) disabled, Delete
/// starts disabled until [`sync_layer_toolbar`] says otherwise.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.root` doesn't exist.
/// On failure the partly built toolbar is removed again.
pub fn insert_layer_toolbar(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
) -> Result<LayerToolbar, WidgetError> {
    let mut node = Node::new(Role::Toolbar);
    node.set_label(LAYER_TOOLBAR_LABEL);
    node.set_orientation(Orientation::Horizontal);
    let root = tree.insert(
        panel.root,
        toolbar_style(scales),
        node,
        WidgetKind::Container,
    )?;
    let built = build(tree, root, scales)
        .and_then(|toolbar| crate::panel::refresh_panel_floor(tree, panel).map(|()| toolbar));
    if built.is_err() {
        let _ = tree.remove(root);
    }
    built
}

fn build(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    scales: &Scales,
) -> Result<LayerToolbar, WidgetError> {
    let mut buttons = [(LayerAction::Link, root); LayerAction::ALL.len()];
    for (slot, action) in buttons.iter_mut().zip(LayerAction::ALL) {
        let id = widgets::insert_icon_button(
            tree,
            root,
            scales,
            action.name(),
            action.icon(),
            LAYER_TOOLBAR_ICON_SIZE,
        )?;
        let disabled = action.is_placeholder() || action == LayerAction::Delete;
        if disabled {
            widgets::set_button_disabled(tree, id, true)?;
        }
        let text = if action == LayerAction::Delete {
            DELETE_UNAVAILABLE
        } else {
            action.description()
        };
        widgets::set_button_description(tree, id, Some(text))?;
        *slot = (action, id);
    }
    Ok(LayerToolbar { root, buttons })
}

/// Mirrors what the document allows into the toolbar: Delete is enabled
/// exactly when `delete_enabled` (the app's own rule — the command would
/// not refuse), with its description saying why when it is not. Link, Fx
/// and Mask are never touched. Returns whether anything changed.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// if a button is missing or is not a button.
pub fn sync_layer_toolbar(
    tree: &mut WidgetTree<WidgetKind>,
    toolbar: &LayerToolbar,
    delete_enabled: bool,
) -> Result<bool, WidgetError> {
    let Some(button) = toolbar.button_for(LayerAction::Delete) else {
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
        LayerAction::Delete.description()
    } else {
        DELETE_UNAVAILABLE
    };
    changed |= widgets::set_button_description(tree, button, Some(text))?;
    Ok(changed)
}

/// Points the hover tint at `hovered` (a button of this toolbar) and
/// clears it from every other one; `None` clears all. Returns whether
/// anything changed.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// if a button is missing or is not a button.
pub fn set_layer_toolbar_hover(
    tree: &mut WidgetTree<WidgetKind>,
    toolbar: &LayerToolbar,
    hovered: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    for (_, id) in toolbar.buttons {
        changed |= widgets::set_button_hovered(tree, id, hovered == Some(id))?;
    }
    Ok(changed)
}

/// Whether `id` is the toolbar or inside it.
#[must_use]
pub fn layer_toolbar_contains(
    tree: &WidgetTree<WidgetKind>,
    toolbar: &LayerToolbar,
    id: WidgetId,
) -> bool {
    tree.is_within(toolbar.root, id)
}

/// Whether `id` is one of the Layer actions toolbar's buttons: a button
/// whose parent is a `Role::Toolbar` labelled [`LAYER_TOOLBAR_LABEL`].
/// Read off the tree, for callers that hold only the tree.
#[must_use]
pub fn is_layer_toolbar_button(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    matches!(tree.payload(id), Some(WidgetKind::Button(state)) if state.icon.is_some())
        && tree
            .parent(id)
            .and_then(|parent| tree.accessibility(parent))
            .is_some_and(|node| {
                node.role() == Role::Toolbar && node.label() == Some(LAYER_TOOLBAR_LABEL)
            })
}

#[cfg(test)]
mod tests {
    use super::{
        DELETE_UNAVAILABLE, LAYER_TOOLBAR_LABEL, LayerAction, is_layer_toolbar_button,
        layer_toolbar_height, set_layer_toolbar_hover, sync_layer_toolbar,
    };
    use crate::layers_panel::populate_layers_panel;
    use crate::workspace::build_workspace;
    use aurora_doc::LayerTree;
    use aurora_widgets::widgets::WidgetKind;

    fn scales() -> aurora_theme::Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn state_of(
        ws: &crate::Workspace,
        action: LayerAction,
    ) -> (
        bool,
        Option<String>,
        Option<aurora_widgets::icons::IconKind>,
    ) {
        let Some(id) = ws.layer_toolbar.button_for(action) else {
            unreachable!("every action has a button");
        };
        let Some(WidgetKind::Button(state)) = ws.tree.payload(id) else {
            unreachable!("a button");
        };
        (
            state.disabled,
            state.description.clone(),
            state.icon.map(|(kind, _)| kind),
        )
    }

    #[test]
    fn seven_buttons_in_order_with_the_right_icons_names_and_descriptions() {
        let ws = build_workspace(&scales());
        let Some(node) = ws.tree.accessibility(ws.layer_toolbar.root) else {
            unreachable!("built");
        };
        assert_eq!(node.role(), accesskit::Role::Toolbar);
        assert_eq!(node.label(), Some(LAYER_TOOLBAR_LABEL));
        assert_eq!(ws.tree.parent(ws.layer_toolbar.root), Some(ws.layers.root));
        let children = ws.tree.children(ws.layer_toolbar.root).unwrap_or_default();
        let order = [
            LayerAction::Link,
            LayerAction::Fx,
            LayerAction::Mask,
            LayerAction::Adjustment,
            LayerAction::Group,
            LayerAction::NewLayer,
            LayerAction::Delete,
        ];
        assert_eq!(children.len(), 7);
        let icons = [
            aurora_widgets::icons::IconKind::Link,
            aurora_widgets::icons::IconKind::Fx,
            aurora_widgets::icons::IconKind::Mask,
            aurora_widgets::icons::IconKind::Adjustment,
            aurora_widgets::icons::IconKind::Group,
            aurora_widgets::icons::IconKind::NewLayer,
            aurora_widgets::icons::IconKind::Delete,
        ];
        for ((action, icon), child) in order.iter().zip(icons).zip(children) {
            assert_eq!(ws.layer_toolbar.action_for(*child), Some(*action));
            let (_, description, kind) = state_of(&ws, *action);
            assert_eq!(kind, Some(icon), "{action:?}");
            assert!(
                description.is_some_and(|text| !text.is_empty()),
                "{action:?}"
            );
            let Some(node) = ws.tree.accessibility(*child) else {
                unreachable!("built");
            };
            assert_eq!(node.label(), Some(action.name()));
            assert!(is_layer_toolbar_button(&ws.tree, *child));
        }
        assert!(!is_layer_toolbar_button(&ws.tree, ws.layers.root));
    }

    #[test]
    fn placeholders_are_disabled_with_a_reason_and_have_no_actions() {
        let mut ws = build_workspace(&scales());
        // Even a sync that enables Delete never touches them.
        assert!(sync_layer_toolbar(&mut ws.tree, &ws.layer_toolbar, true).is_ok());
        for action in [LayerAction::Link, LayerAction::Fx, LayerAction::Mask] {
            let (disabled, description, _) = state_of(&ws, action);
            assert!(disabled, "{action:?}");
            assert!(action.is_placeholder());
            assert!(
                description.is_some_and(|text| text.contains("can't") && text.contains("yet")),
                "{action:?} says why"
            );
            let Some(id) = ws.layer_toolbar.button_for(action) else {
                unreachable!("built");
            };
            let Some(node) = ws.tree.accessibility(id) else {
                unreachable!("built");
            };
            assert!(node.is_disabled());
            assert!(!node.supports_action(accesskit::Action::Click));
            assert!(!node.supports_action(accesskit::Action::Focus));
        }
        for action in [
            LayerAction::Adjustment,
            LayerAction::Group,
            LayerAction::NewLayer,
        ] {
            assert!(!state_of(&ws, action).0, "{action:?} is enabled");
            assert!(!action.is_placeholder());
        }
    }

    #[test]
    fn delete_follows_the_sync_and_says_why_when_off() {
        let mut ws = build_workspace(&scales());
        assert!(state_of(&ws, LayerAction::Delete).0, "starts disabled");
        assert_eq!(
            state_of(&ws, LayerAction::Delete).1.as_deref(),
            Some(DELETE_UNAVAILABLE)
        );
        assert!(matches!(
            sync_layer_toolbar(&mut ws.tree, &ws.layer_toolbar, true),
            Ok(true)
        ));
        assert!(!state_of(&ws, LayerAction::Delete).0);
        assert_eq!(
            state_of(&ws, LayerAction::Delete).1.as_deref(),
            Some(LayerAction::Delete.description())
        );
        assert!(matches!(
            sync_layer_toolbar(&mut ws.tree, &ws.layer_toolbar, true),
            Ok(false)
        ));
        assert!(matches!(
            sync_layer_toolbar(&mut ws.tree, &ws.layer_toolbar, false),
            Ok(true)
        ));
        assert!(state_of(&ws, LayerAction::Delete).0);
    }

    #[test]
    fn the_toolbar_survives_repopulation_sits_last_and_joins_the_floor() {
        let scales = scales();
        let mut ws = build_workspace(&scales);
        let ids: Vec<_> = ws.layer_toolbar.buttons.iter().map(|(_, id)| *id).collect();
        let layers = LayerTree::new();
        for _ in 0..3 {
            if let Err(err) = populate_layers_panel(&mut ws.tree, ws.layers, &scales, &layers) {
                unreachable!("{err:?}");
            }
        }
        assert!(ws.tree.contains(ws.layer_toolbar.root));
        assert!(ids.iter().all(|id| ws.tree.contains(*id)));
        let children = ws.tree.children(ws.layers.root).unwrap_or_default();
        assert_eq!(
            children.last(),
            Some(&ws.layer_toolbar.root),
            "after the list"
        );
        ws.tree.compute_layout(1600.0, 900.0);
        let (Some(panel), Some(bar), Some(view)) = (
            ws.tree.bounds(ws.layers.root),
            ws.tree.bounds(ws.layer_toolbar.root),
            ws.tree.bounds(ws.layers.viewport),
        ) else {
            unreachable!("laid out");
        };
        assert_eq!(bar.height, super::toolbar_px(&scales));
        let _ = layer_toolbar_height(&scales);
        assert!(bar.y >= view.y + i64::from(view.height), "below the list");
        assert!(bar.y + i64::from(bar.height) <= panel.y + i64::from(panel.height));
        // Right-aligned: the last button ends at the toolbar's padded edge.
        let Some(last) = ids.last().and_then(|id| ws.tree.bounds(*id)) else {
            unreachable!("laid out");
        };
        assert!(last.x + i64::from(last.width) <= bar.x + i64::from(bar.width));
        let first = ids
            .first()
            .and_then(|id| ws.tree.bounds(*id))
            .map_or(i64::MIN, |b| b.x);
        assert!(
            first >= bar.x,
            "all seven fit the default rail: {first} vs {bar:?}"
        );
        // A collapsed panel hides it with the body.
        if let Err(err) = crate::set_panel_collapsed(&mut ws.tree, ws.layers, true) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);
        assert!(
            ws.tree
                .bounds(ws.layer_toolbar.root)
                .is_none_or(|b| b.height == 0)
        );
    }

    #[test]
    fn hover_marks_one_button_and_clears_the_rest() {
        let mut ws = build_workspace(&scales());
        let Some(group) = ws.layer_toolbar.button_for(LayerAction::Group) else {
            unreachable!("built");
        };
        let hovered = |ws: &crate::Workspace| {
            ws.layer_toolbar
                .buttons
                .iter()
                .filter(|(_, id)| {
                    matches!(ws.tree.payload(*id), Some(WidgetKind::Button(s)) if s.hovered)
                })
                .count()
        };
        assert!(matches!(
            set_layer_toolbar_hover(&mut ws.tree, &ws.layer_toolbar, Some(group)),
            Ok(true)
        ));
        assert_eq!(hovered(&ws), 1);
        assert!(matches!(
            set_layer_toolbar_hover(&mut ws.tree, &ws.layer_toolbar, None),
            Ok(true)
        ));
        assert_eq!(hovered(&ws), 0);
    }

    #[test]
    fn tab_order_runs_list_then_toolbar_enabled_buttons() {
        let mut ws = build_workspace(&scales());
        let _ = sync_layer_toolbar(&mut ws.tree, &ws.layer_toolbar, true);
        let mut focus = aurora_widgets::FocusManager::new();
        let mut visited = Vec::new();
        for _ in 0..40 {
            match focus.focus_next(&mut ws.tree) {
                Some(id) => visited.push(id),
                None => break,
            }
        }
        let pos = |id| visited.iter().position(|v| *v == id);
        let (Some(layers), Some(adj), Some(group), Some(new), Some(del)) = (
            pos(ws.layers.root),
            ws.layer_toolbar
                .button_for(LayerAction::Adjustment)
                .and_then(pos),
            ws.layer_toolbar
                .button_for(LayerAction::Group)
                .and_then(pos),
            ws.layer_toolbar
                .button_for(LayerAction::NewLayer)
                .and_then(pos),
            ws.layer_toolbar
                .button_for(LayerAction::Delete)
                .and_then(pos),
        ) else {
            unreachable!("all are tab stops");
        };
        assert!(layers < adj && adj < group && group < new && new < del);
        for placeholder in [LayerAction::Link, LayerAction::Fx, LayerAction::Mask] {
            assert!(
                ws.layer_toolbar
                    .button_for(placeholder)
                    .and_then(pos)
                    .is_none()
            );
        }
    }
}
