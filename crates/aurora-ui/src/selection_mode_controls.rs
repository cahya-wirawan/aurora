//! The options bar's **selection mode** control (0.215.0): Photoshop's
//! New / Add / Subtract / Intersect buttons for the five selection tools
//! (Marquee, Elliptical Marquee, Lasso, Polygonal Lasso, Magic Wand).
//!
//! Four checkboxes in one group row inside the options strip, kept a radio
//! group by the app (whatever a checkbox did, [`sync_selection_mode_controls`]
//! re-mirrors the one mode in force, so exactly one is ever checked): a
//! toggle-button row is taller than the bar's row (see
//! [`crate::fill_controls`]). The group is shown only for a selection tool
//! and is hidden by layout and from assistive technology otherwise.
//!
//! **This module owns no setting**: `aurora-app` passes the mode in force
//! and applies the checkbox that changed. Nothing here is document state.
//! No new design tokens (`spacing.sm` gaps).

use accesskit::Toggled;
use aurora_doc::SelectionMode;
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{Display, FlexDirection, Size, Style};

use crate::tool::Tool;
use crate::tool_controls::{
    OptionsItem, ToolControls, set_item_shown, set_node_hidden, set_options_item,
};

/// The group's accessible name.
pub const SELECTION_MODE_LABEL: &str = "Selection mode";

/// The widgets of the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionModeControls {
    /// The group's row.
    pub group: WidgetId,
    /// "New": replace the selection.
    pub replace: WidgetId,
    /// "Add".
    pub add: WidgetId,
    /// "Subtract".
    pub subtract: WidgetId,
    /// "Intersect".
    pub intersect: WidgetId,
}

impl SelectionModeControls {
    /// The checkbox of `mode`.
    #[must_use]
    pub const fn checkbox(&self, mode: SelectionMode) -> WidgetId {
        match mode {
            SelectionMode::Replace => self.replace,
            SelectionMode::Add => self.add,
            SelectionMode::Subtract => self.subtract,
            SelectionMode::Intersect => self.intersect,
        }
    }

    /// The mode whose checkbox is `id`, if it is one of the four.
    #[must_use]
    pub fn mode_of(&self, id: WidgetId) -> Option<SelectionMode> {
        SelectionMode::ALL
            .into_iter()
            .find(|&mode| self.checkbox(mode) == id)
    }
}

/// The checkbox label of `mode`.
#[must_use]
pub const fn selection_mode_label(mode: SelectionMode) -> &'static str {
    match mode {
        SelectionMode::Replace => "New",
        SelectionMode::Add => "Add",
        SelectionMode::Subtract => "Subtract",
        SelectionMode::Intersect => "Intersect",
    }
}

fn group_style(scales: &Scales) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let gap = length(scales.spacing.sm as f32);
    Style {
        display: Display::None,
        flex_direction: FlexDirection::Row,
        flex_shrink: 0.0,
        align_items: Some(taffy::AlignItems::CENTER),
        gap: Size {
            width: gap,
            height: gap,
        },
        ..Default::default()
    }
}

/// Adds the group as the last child of `options` (hidden).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `options` doesn't exist.
pub fn insert_selection_mode_controls(
    tree: &mut WidgetTree<WidgetKind>,
    options: WidgetId,
    scales: &Scales,
) -> Result<SelectionModeControls, WidgetError> {
    let group = widgets::insert_container(tree, options, group_style(scales))?;
    let mut node = accesskit::Node::new(accesskit::Role::Group);
    node.set_label(SELECTION_MODE_LABEL);
    tree.set_accessibility(group, node)?;
    let mut ids = [group; 4];
    for (slot, mode) in ids.iter_mut().zip(SelectionMode::ALL) {
        *slot = widgets::insert_checkbox(tree, group, scales, selection_mode_label(mode))?;
        set_options_item(tree, scales, *slot, OptionsItem::Check, true)?;
    }
    let [replace, add, subtract, intersect] = ids;
    widgets::set_checkbox_checked(tree, replace, Toggled::True)?;
    set_node_hidden(tree, group, true)?;
    Ok(SelectionModeControls {
        group,
        replace,
        add,
        subtract,
        intersect,
    })
}

/// Shows the group for a selection tool (hidden for any other), and
/// checks exactly the checkbox of `mode`. Never inserts or removes a
/// widget, so held ids stay valid. Returns whether anything changed.
///
/// # Errors
///
/// Returns whatever the widget setters return for an id that is not the
/// kind `controls` says it is.
pub fn sync_selection_mode_controls(
    tree: &mut WidgetTree<WidgetKind>,
    controls: &ToolControls,
    tool: Tool,
    mode: SelectionMode,
) -> Result<bool, WidgetError> {
    let modes = &controls.modes;
    let shown = tool.is_selection_tool();
    let mut changed = set_item_shown(tree, modes.group, shown)?;
    changed |= set_node_hidden(tree, modes.group, !shown)?;
    for candidate in SelectionMode::ALL {
        let id = modes.checkbox(candidate);
        let want = if candidate == mode {
            Toggled::True
        } else {
            Toggled::False
        };
        match tree.payload(id) {
            Some(WidgetKind::Checkbox(state)) if state.checked == want => {}
            Some(WidgetKind::Checkbox(_)) => {
                widgets::set_checkbox_checked(tree, id, want)?;
                changed = true;
            }
            Some(_) => return Err(WidgetError::WrongWidgetKind(id)),
            None => return Err(WidgetError::UnknownWidget(id)),
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::{SELECTION_MODE_LABEL, selection_mode_label, sync_selection_mode_controls};
    use crate::tool::Tool;
    use crate::tool_controls::{ToolControls, insert_tool_controls, tool_controls_contains};
    use crate::workspace::build_workspace;
    use aurora_doc::SelectionMode;
    use aurora_theme::Scales;
    use aurora_widgets::widgets::WidgetKind;
    use aurora_widgets::{WidgetId, WidgetTree};
    use taffy::Display;

    fn built() -> (crate::Workspace, ToolControls) {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        let scales = match Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut ws = build_workspace(&scales);
        let controls =
            match insert_tool_controls(&mut ws.tree, ws.options_bar, ws.properties, &scales) {
                Ok(controls) => controls,
                Err(err) => unreachable!("{err:?}"),
            };
        ws.tree.compute_layout(1600.0, 900.0);
        (ws, controls)
    }

    fn sync(ws: &mut crate::Workspace, c: &ToolControls, tool: Tool, mode: SelectionMode) {
        if let Err(err) = sync_selection_mode_controls(&mut ws.tree, c, tool, mode) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);
    }

    fn shown(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
        tree.style(id).is_some_and(|s| s.display != Display::None)
    }

    fn checked(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
        matches!(
            tree.payload(id),
            Some(WidgetKind::Checkbox(state)) if state.checked == accesskit::Toggled::True
        )
    }

    #[test]
    fn only_the_six_selection_tools_show_the_group() {
        let (mut ws, c) = built();
        for tool in Tool::ALL {
            sync(&mut ws, &c, tool, SelectionMode::Replace);
            let want = matches!(
                tool,
                Tool::MarqueeSelect
                    | Tool::EllipticalMarquee
                    | Tool::Lasso
                    | Tool::PolygonalLasso
                    | Tool::MagicWand
                    | Tool::QuickSelect
            );
            assert_eq!(tool.is_selection_tool(), want, "{tool:?}");
            assert_eq!(shown(&ws.tree, c.modes.group), want, "{tool:?}");
            let hidden = ws
                .tree
                .accessibility(c.modes.group)
                .is_some_and(accesskit::Node::is_hidden);
            assert_eq!(hidden, !want, "{tool:?}: hidden from AT");
        }
    }

    #[test]
    fn exactly_the_checkbox_of_the_mode_in_force_is_checked_and_a_shown_group_has_size() {
        let (mut ws, c) = built();
        for mode in SelectionMode::ALL {
            sync(&mut ws, &c, Tool::Lasso, mode);
            for other in SelectionMode::ALL {
                assert_eq!(
                    checked(&ws.tree, c.modes.checkbox(other)),
                    other == mode,
                    "{mode:?} / {other:?}"
                );
            }
        }
        for mode in SelectionMode::ALL {
            let id = c.modes.checkbox(mode);
            let rect = ws.tree.bounds(id);
            assert!(
                rect.is_some_and(|r| r.width > 0 && r.height > 0),
                "{mode:?} has size: {rect:?}"
            );
            assert!(tool_controls_contains(&ws.tree, &c, id));
            assert_eq!(c.modes.mode_of(id), Some(mode));
        }
        assert_eq!(c.modes.mode_of(c.radius), None);
    }

    #[test]
    fn labels_are_the_photoshop_names_and_the_group_has_a_name() {
        assert_eq!(selection_mode_label(SelectionMode::Replace), "New");
        assert_eq!(selection_mode_label(SelectionMode::Add), "Add");
        assert_eq!(selection_mode_label(SelectionMode::Subtract), "Subtract");
        assert_eq!(selection_mode_label(SelectionMode::Intersect), "Intersect");
        assert_eq!(SELECTION_MODE_LABEL, "Selection mode");
        let (ws, c) = built();
        let node = ws.tree.accessibility(c.modes.group);
        assert_eq!(
            node.map(accesskit::Node::role),
            Some(accesskit::Role::Group)
        );
        assert_eq!(
            node.and_then(accesskit::Node::label),
            Some(SELECTION_MODE_LABEL)
        );
    }
}
