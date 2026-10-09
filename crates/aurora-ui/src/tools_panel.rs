//! The left tools panel (0.160.0): a narrow, vertical, non-scrolling strip
//! on the canvas area's left edge holding one text-labelled toggle button
//! per [`Tool`] — the first of the workspace rounds, following
//! Photoshop's layout convention (tools on the left, the active tool's
//! options across the top) in Aurora's own tokens.
//!
//! **Text labels, not icons** — the design owner's call (Cahya,
//! 2026-10-09): real icons wait for an icon set. Every size here comes
//! from spacing tokens and, in the running app, from text measurement
//! (`aurora_widgets::compute_text_layout` measures a toggle button's
//! label); nothing is a literal.
//!
//! **This module owns no tool state.** Which tool is active is
//! `aurora-app`'s (`App::tool`); a click or an assistive-technology
//! `Click` on a button is routed there and goes through the same
//! `SelectTool` command a shortcut uses, and the app then mirrors the
//! result back with [`sync_tools_panel`]. The buttons behave as a radio
//! group: exactly one is "on" (accesskit `toggled`), clicking the
//! selected one leaves it selected.
//!
//! **Keyboard: each button is its own `Tab` stop**, in [`Tool::ALL`]
//! order — the cheap option, chosen over the arrow-key toolbar pattern
//! (one stop, roving focus) because the toolkit has no roving-focus
//! mechanism yet; `Space`/`Enter` on a focused button clicks it.

use accesskit::{Node, Orientation, Role};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{FlexDirection, Rect as LayoutRect, Size, Style};

use crate::tool::Tool;

/// The tools panel's accessible label.
pub const TOOLS_PANEL_LABEL: &str = "Tools";

/// The strip and its buttons, one per [`Tool::ALL`] entry, in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolsPanel {
    /// The `Role::Toolbar` strip.
    pub root: WidgetId,
    /// `(tool, its toggle button)`, in [`Tool::ALL`] order.
    pub buttons: [(Tool, WidgetId); Tool::ALL.len()],
}

impl ToolsPanel {
    /// The tool button `id` selects, if `id` is one of this panel's buttons.
    #[must_use]
    pub fn tool_for(&self, id: WidgetId) -> Option<Tool> {
        self.buttons
            .iter()
            .find(|(_, button)| *button == id)
            .map(|(tool, _)| *tool)
    }

    /// `tool`'s button.
    #[must_use]
    pub fn button_for(&self, tool: Tool) -> Option<WidgetId> {
        self.buttons
            .iter()
            .find(|(candidate, _)| *candidate == tool)
            .map(|(_, button)| *button)
    }
}

/// The strip's style: a column, never shrinking below its content (so a
/// narrow window squeezes the canvas, never the tool labels), padded and
/// gapped by `spacing.xs`. Its width is its widest button's.
pub(crate) fn tools_style(scales: &Scales) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let pad = length(scales.spacing.xs as f32);
    Style {
        flex_direction: FlexDirection::Column,
        flex_shrink: 0.0,
        gap: Size {
            width: pad,
            height: pad,
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

/// Adds the tools panel as the last child of `parent`, with `selected`'s
/// button on.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist. On
/// failure the partly built strip is removed again.
pub fn insert_tools_panel(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    selected: Tool,
) -> Result<ToolsPanel, WidgetError> {
    let mut node = Node::new(Role::Toolbar);
    node.set_label(TOOLS_PANEL_LABEL);
    node.set_orientation(Orientation::Vertical);
    let root = tree.insert(parent, tools_style(scales), node, WidgetKind::Container)?;
    let built = build(tree, root, scales, selected);
    if built.is_err() {
        let _ = tree.remove(root);
    }
    built
}

fn build(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    scales: &Scales,
    selected: Tool,
) -> Result<ToolsPanel, WidgetError> {
    let mut buttons = [(Tool::default(), root); Tool::ALL.len()];
    for (slot, tool) in buttons.iter_mut().zip(Tool::ALL) {
        let id = widgets::insert_toggle_button(tree, root, scales, tool.label(), tool == selected)?;
        *slot = (tool, id);
    }
    Ok(ToolsPanel { root, buttons })
}

/// Turns `selected`'s button on and every other one off, returning
/// whether anything changed.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// if a button is missing or is not a toggle button.
pub fn sync_tools_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: &ToolsPanel,
    selected: Tool,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    for (tool, id) in panel.buttons {
        changed |= widgets::set_button_toggled(tree, id, tool == selected)?;
    }
    Ok(changed)
}

/// The tool whose button is on, or `None` if none is (or the tree has no
/// such buttons).
#[must_use]
pub fn selected_tool(tree: &WidgetTree<WidgetKind>, panel: &ToolsPanel) -> Option<Tool> {
    panel
        .buttons
        .iter()
        .find_map(|(tool, id)| match tree.payload(*id) {
            Some(WidgetKind::Button(state)) if state.toggled == Some(true) => Some(*tool),
            _ => None,
        })
}

/// Whether `id` is the tools panel or inside it.
#[must_use]
pub fn tools_panel_contains(
    tree: &WidgetTree<WidgetKind>,
    panel: &ToolsPanel,
    id: WidgetId,
) -> bool {
    tree.is_within(panel.root, id)
}

#[cfg(test)]
mod tests {
    use super::{TOOLS_PANEL_LABEL, selected_tool, sync_tools_panel, tools_panel_contains};
    use crate::tool::Tool;
    use crate::workspace::build_workspace;

    fn scales() -> aurora_theme::Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn the_panel_is_a_labelled_vertical_toolbar_with_one_toggle_per_tool() {
        let ws = build_workspace(&scales());
        let Some(node) = ws.tree.accessibility(ws.tools.root) else {
            unreachable!("built");
        };
        assert_eq!(node.role(), accesskit::Role::Toolbar);
        assert_eq!(node.label(), Some(TOOLS_PANEL_LABEL));
        assert_eq!(node.orientation(), Some(accesskit::Orientation::Vertical));
        let children = ws.tree.children(ws.tools.root).unwrap_or_default();
        assert_eq!(children.len(), Tool::ALL.len());
        for ((tool, id), child) in ws.tools.buttons.iter().zip(children) {
            assert_eq!(id, child, "buttons are in Tool::ALL order");
            let Some(node) = ws.tree.accessibility(*id) else {
                unreachable!("built");
            };
            assert_eq!(node.role(), accesskit::Role::Button);
            assert_eq!(node.label(), Some(tool.label()));
            assert!(node.supports_action(accesskit::Action::Click));
            assert!(node.supports_action(accesskit::Action::Focus));
            let expected = if *tool == Tool::default() {
                accesskit::Toggled::True
            } else {
                accesskit::Toggled::False
            };
            assert_eq!(node.toggled(), Some(expected), "{tool:?}");
            assert_eq!(ws.tools.tool_for(*id), Some(*tool));
            assert_eq!(ws.tools.button_for(*tool), Some(*id));
            assert!(tools_panel_contains(&ws.tree, &ws.tools, *id));
        }
        assert_eq!(ws.tools.tool_for(ws.canvas_area), None);
        assert_eq!(selected_tool(&ws.tree, &ws.tools), Some(Tool::default()));
    }

    #[test]
    fn syncing_leaves_exactly_one_button_on() {
        let mut ws = build_workspace(&scales());
        for tool in Tool::ALL {
            let changed = sync_tools_panel(&mut ws.tree, &ws.tools, tool);
            assert!(
                matches!(changed, Ok(true)),
                "each step switches from a different tool"
            );
            assert_eq!(selected_tool(&ws.tree, &ws.tools), Some(tool));
            let on = ws
                .tools
                .buttons
                .iter()
                .filter(|(_, id)| {
                    ws.tree
                        .accessibility(*id)
                        .and_then(accesskit::Node::toggled)
                        == Some(accesskit::Toggled::True)
                })
                .count();
            assert_eq!(on, 1, "{tool:?}");
        }
        assert!(
            matches!(
                sync_tools_panel(&mut ws.tree, &ws.tools, Tool::Eraser),
                Ok(false)
            ),
            "syncing the same tool again changes nothing"
        );
    }
}
