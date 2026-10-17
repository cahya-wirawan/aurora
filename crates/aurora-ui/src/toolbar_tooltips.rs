//! The text a tooltip shows for the icon-only toolbar buttons (0.257.0):
//! the Tools panel, the Layers bottom toolbar and the Paths and Channels
//! foot toolbars.
//!
//! **What this module is.** The *content contract*:
//! [`tooltip_text_for`] answers, for any widget of the workspace, the one
//! string a tooltip on it would show (or `None`). Since 0.262.0 the live
//! app shows it: [`crate::tooltip_hover`] tracks the pointer, times the
//! hover delay and places the toolkit's `Tooltip` at its measured width
//! inside the window.
//!
//! **No double announcement.** The shown tooltip is a `Role::Tooltip`
//! child of the button (`tooltip.rs`'s accessibility vocabulary), never
//! part of the owner's name, and the app hides that node from assistive
//! technology (`Tooltip::with_assistive_hidden`) because the button
//! already carries the same words as its accessible *description* (the
//! Layers, Paths and Channels buttons: the tooltip IS the description) or
//! a deliberately different, longer one (the Tools panel's "Brush tool,
//! shortcut B" against "Brush (B)"). A tooltip never copies its text into
//! the owner's description.
//!
//! **Disabled buttons** show their *reason* (the button's description,
//! "Layer effects can't be added from here yet"), never a bare action
//! name; a button with no description says nothing ([`None`]).

use aurora_widgets::widgets::WidgetKind;
use aurora_widgets::{WidgetId, WidgetTree};

use crate::tool::Tool;
use crate::workspace::Workspace;

/// A tool button's tooltip: its name and its shortcut, "Brush (B)",
/// "Paint Bucket (Shift+G)".
#[must_use]
pub fn tool_tooltip_text(tool: Tool) -> String {
    format!("{} ({})", tool.label(), tool.shortcut_text())
}

/// The Swap Colours button's tooltip.
pub const SWAP_COLOURS_TOOLTIP: &str = "Swap Colours (X)";

/// The Default Colours button's tooltip.
pub const DEFAULT_COLOURS_TOOLTIP: &str = "Default Colours (D)";

/// The description a Button currently carries: for the toolbar buttons
/// that is what the action does, or why it is unavailable.
fn button_description(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Option<String> {
    match tree.payload(id) {
        Some(WidgetKind::Button(state)) => state.description.clone().filter(|d| !d.is_empty()),
        _ => None,
    }
}

/// The tooltip text for widget `id`: a Tools panel button's
/// [`tool_tooltip_text`]; the Swap/Default Colours buttons' fixed texts;
/// the document strip's "More documents" button; a Layers, Paths or
/// Channels toolbar button's current description (the reason, while it is
/// disabled); `None` for anything else. The menu bar's text buttons and
/// the tabs' close buttons have no tooltip (see [`crate::tooltip_hover`]).
#[must_use]
pub fn tooltip_text_for(workspace: &Workspace, id: WidgetId) -> Option<String> {
    if let Some(tool) = workspace.tools.tool_for(id) {
        return Some(tool_tooltip_text(tool));
    }
    let colours = workspace.tools.colours;
    if id == colours.swap {
        return Some(SWAP_COLOURS_TOOLTIP.to_owned());
    }
    if id == colours.defaults {
        return Some(DEFAULT_COLOURS_TOOLTIP.to_owned());
    }
    if id == workspace.document_overflow {
        return Some(crate::document_tabs::DOCUMENT_OVERFLOW_LABEL.to_owned());
    }
    let tree = &workspace.tree;
    let toolbar_button = crate::is_layer_toolbar_button(tree, id)
        || crate::is_paths_toolbar_button(tree, id)
        || crate::is_channels_toolbar_button(tree, id);
    if toolbar_button {
        return button_description(tree, id);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{tool_tooltip_text, tooltip_text_for};
    use crate::layer_toolbar::LayerAction;
    use crate::tool::Tool;
    use crate::workspace::{Workspace, build_workspace};

    fn workspace() -> Workspace {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => build_workspace(&scales),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// Every tool button has exactly one tooltip text, its name and its
    /// shortcut, unique among the tools.
    #[test]
    fn every_tool_button_has_one_tooltip_text_equal_to_name_and_shortcut() {
        let ws = workspace();
        let mut seen = Vec::new();
        for tool in Tool::ALL {
            let Some((_, button)) = ws.tools.buttons.iter().find(|(t, _)| *t == tool) else {
                unreachable!("{tool:?} has a button");
            };
            let text = tooltip_text_for(&ws, *button);
            assert_eq!(
                text.as_deref(),
                Some(format!("{} ({})", tool.label(), tool.shortcut_text()).as_str()),
                "{tool:?}"
            );
            assert_eq!(text, Some(tool_tooltip_text(tool)));
            seen.push(tool_tooltip_text(tool));
        }
        seen.sort();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before, "no two tools share a tooltip");
        assert_eq!(tool_tooltip_text(Tool::Brush), "Brush (B)");
        assert_eq!(
            tool_tooltip_text(Tool::PaintBucket),
            "Paint Bucket (Shift+G)"
        );
    }

    /// The tooltip is a visual hint, the description is what a screen
    /// reader reads: they must differ, or a wired tooltip would say the
    /// same words twice.
    #[test]
    fn the_tool_tooltip_is_not_the_accessible_description() {
        let ws = workspace();
        for (tool, button) in ws.tools.buttons {
            let description = ws
                .tree
                .accessibility(button)
                .and_then(|n| n.description().map(str::to_owned))
                .unwrap_or_default();
            assert!(!description.is_empty(), "{tool:?}");
            assert_ne!(description, tool_tooltip_text(tool), "{tool:?}");
        }
    }

    /// The three disabled Layers placeholders show their reason; every
    /// Layers button's tooltip is its description.
    #[test]
    fn layer_toolbar_buttons_show_their_description_and_placeholders_their_reason() {
        let ws = workspace();
        for (action, button) in ws.layer_toolbar.buttons {
            let text = tooltip_text_for(&ws, button);
            let live = ws
                .tree
                .accessibility(button)
                .and_then(|n| n.description().map(str::to_owned));
            assert_eq!(
                text, live,
                "{action:?}: the tooltip is the current description"
            );
            if action == LayerAction::Delete {
                // Delete is disabled on the fresh document-less workspace:
                // its reason, not the action's name.
                assert!(text.as_deref().unwrap_or_default().starts_with("Can't"));
            } else {
                assert_eq!(text.as_deref(), Some(action.description()), "{action:?}");
            }
            if action.is_placeholder() {
                let text = text.unwrap_or_default();
                assert!(text.contains("can't"), "a reason, not a name: {text}");
            }
        }
        assert!(LayerAction::ALL.iter().any(|a| a.is_placeholder()));
    }

    /// Paths and Channels foot buttons: the tooltip is whatever the button
    /// currently describes (their reason while disabled), nothing when it
    /// describes nothing.
    #[test]
    fn paths_and_channels_buttons_follow_their_current_description() {
        let mut ws = workspace();
        for (action, button) in ws.paths_state.toolbar.buttons {
            let live = ws
                .tree
                .accessibility(button)
                .and_then(|n| n.description().map(str::to_owned));
            assert_eq!(tooltip_text_for(&ws, button), live, "{action:?}");
        }
        for (action, button) in ws.channels_state.toolbar.buttons {
            let live = ws
                .tree
                .accessibility(button)
                .and_then(|n| n.description().map(str::to_owned));
            assert_eq!(tooltip_text_for(&ws, button), live, "{action:?}");
        }
        // A button whose description is cleared has no tooltip.
        let Some((_, button)) = ws.paths_state.toolbar.buttons.first().copied() else {
            unreachable!("the Paths toolbar has buttons");
        };
        if let Err(err) =
            aurora_widgets::widgets::set_button_description(&mut ws.tree, button, None)
        {
            unreachable!("{err:?}");
        }
        assert_eq!(tooltip_text_for(&ws, button), None);
    }

    /// Not a toolbar button: no tooltip.
    #[test]
    fn other_widgets_have_no_toolbar_tooltip() {
        let ws = workspace();
        assert_eq!(tooltip_text_for(&ws, ws.root), None);
        assert_eq!(tooltip_text_for(&ws, ws.rail), None);
    }
}
