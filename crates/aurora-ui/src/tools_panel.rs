//! The left tools panel: a narrow, non-scrolling strip on the canvas
//! area's left edge holding one flat **icon** toggle button per [`Tool`],
//! laid out the way Photoshop's Tools panel is (reference
//! `images/photoshop-workspace-01.jpg`, section C) in Aurora's own
//! tokens.
//!
//! **0.207.0 rebuild.** The panel began (0.160.0) as a column of
//! text-labelled toggle buttons, the design owner's call until an icon
//! set existed (Cahya, 2026-10-09). The set exists since 0.194.0, so the
//! labels are gone: each button is an
//! [`insert_icon_toggle_button`](aurora_widgets::widgets::insert_icon_toggle_button)
//! ([`TOOL_ICON_SIZE`], a 32 px square: the 24 px icon plus `spacing.xxs`
//! each side), flat at rest, tinted `state.hover_overlay` when the owner
//! says it is hovered ([`set_tools_panel_hover`]) and `state.active_overlay`
//! while pressed, and **on** for the selected tool: filled `accent.primary`
//! with the icon in `icon.on_accent` (so the selected tool differs by a
//! filled square, not by hue alone, and by the accesskit `toggled` state).
//!
//! **Two columns.** Photoshop packs its tools two to a row, and the panel
//! can do so with existing tokens only: the strip is `2 x 32 px` plus
//! `spacing.xxs` of padding each side, 72 px at the default density
//! (the text-labelled strip was about 140 px: `spacing.xs` each side, the
//! widest label "Marquee Select" and `spacing.md` of button padding). The
//! buttons sit in [`TOOL_GROUPS`], one row per group of at most two, and a
//! group rule ([`widgets::insert_separator`], the menu separator's 1 px
//! `border.default` line) separates the groups: move and select, view,
//! sample, paint (Brush and Pencil), erase (Eraser and Clone Stamp), smudge, fill — Photoshop's own grouping, trimmed to the tools
//! Aurora has (the fill group, Gradient and Paint Bucket, is 0.209.0's: a
//! row of its own under the paint group). A group of one (the Eyedropper) fills the left cell.
//!
//! **The colour swatches (0.208.0).** Below the last tool group, behind
//! one more group rule, sit Photoshop's foreground/background swatches,
//! the swap arrow and the default-colours icon
//! ([`crate::colour_controls`]). The colours themselves are
//! `aurora-app`'s state, not this panel's.
//!
//! **This module owns no tool state.** Which tool is active is
//! `aurora-app`'s (`App::tool`); a click or an assistive-technology
//! `Click` on a button is routed there and goes through the same
//! `SelectTool` command a shortcut uses, and the app then mirrors the
//! result back with [`sync_tools_panel`]. The buttons behave as a radio
//! group: exactly one is "on" (accesskit `toggled`), clicking the
//! selected one leaves it selected. **Accessibility:** every button keeps
//! the tool's full name as its accessible name (never drawn) and a
//! description — "Name tool, shortcut V" — the text a tooltip would show
//! and a screen reader reads after the name.
//!
//! **Keyboard: each button is its own `Tab` stop**, in [`Tool::ALL`]
//! order (the reading order of the grid) — the cheap option, chosen over
//! the arrow-key toolbar pattern (one stop, roving focus) because the
//! toolkit has no roving-focus mechanism yet; `Space`/`Enter` on a
//! focused button clicks it. The separators are not stops.
//!
//! **Hover** is app-driven like the Layers toolbar's: the widget layer
//! tracks no pointer, so `aurora-app` calls [`set_tools_panel_hover`].
//!
//! Every size is a token or derived from one; nothing is a literal. The
//! icons are the provisional set of [`aurora_widgets::icons`] and have
//! not been seen by a human, nor on the Mac. A button's side is the
//! `size.icon_button` token (0.219.0, 32 px;
//! [`aurora_widgets::widgets::icon_button_side`] at [`IconSize::Large`]),
//! shared with the Layers toolbar.

use accesskit::{Node, Orientation, Role};
use aurora_theme::Scales;
use aurora_widgets::icons::IconKind;
use aurora_widgets::widgets::{self, IconSize, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{FlexDirection, Rect as LayoutRect, Size, Style};

use crate::colour_controls::{ColourControls, insert_colour_controls};
use crate::tool::Tool;

/// The colours a fresh panel shows before the app mirrors its own: black
/// over white, Photoshop's defaults.
pub const DEFAULT_FOREGROUND: [f32; 3] = [0.0, 0.0, 0.0];
/// See [`DEFAULT_FOREGROUND`].
pub const DEFAULT_BACKGROUND: [f32; 3] = [1.0, 1.0, 1.0];

/// The tools panel's accessible label.
pub const TOOLS_PANEL_LABEL: &str = "Tools";

/// The icon size of a tool button: the Layers toolbar's.
pub const TOOL_ICON_SIZE: IconSize = IconSize::Large;

/// How many buttons share a row of the grid.
pub const TOOL_COLUMNS: usize = 2;

/// The groups of the strip, top to bottom, each one row of at most
/// [`TOOL_COLUMNS`] buttons, a rule between consecutive groups. Together
/// they hold every tool of [`Tool::ALL`], in that order.
pub const TOOL_GROUPS: [&[Tool]; 10] = [
    &[Tool::Move],
    &[Tool::MarqueeSelect, Tool::EllipticalMarquee],
    &[Tool::Lasso, Tool::PolygonalLasso],
    &[Tool::MagicWand],
    &[Tool::Zoom, Tool::Pan],
    &[Tool::Eyedropper],
    &[Tool::Brush, Tool::Pencil],
    &[Tool::Eraser, Tool::CloneStamp],
    &[Tool::Smudge],
    &[Tool::Gradient, Tool::PaintBucket],
];

/// The icon on `tool`'s button. (`Move` and `Brush` are the icons the
/// Layers lock row already uses for "position" and "image pixels", as in
/// Photoshop.)
#[must_use]
pub const fn tool_icon(tool: Tool) -> IconKind {
    match tool {
        Tool::Move => IconKind::Move,
        Tool::MarqueeSelect => IconKind::Marquee,
        Tool::EllipticalMarquee => IconKind::EllipseMarquee,
        Tool::Lasso => IconKind::Lasso,
        Tool::PolygonalLasso => IconKind::PolygonalLasso,
        Tool::MagicWand => IconKind::MagicWand,
        Tool::Zoom => IconKind::ZoomIn,
        Tool::Pan => IconKind::Hand,
        Tool::Eyedropper => IconKind::Eyedropper,
        Tool::Brush => IconKind::Brush,
        Tool::Eraser => IconKind::Eraser,
        Tool::Pencil => IconKind::Pencil,
        Tool::CloneStamp => IconKind::CloneStamp,
        Tool::Smudge => IconKind::Smudge,
        Tool::Gradient => IconKind::Gradient,
        Tool::PaintBucket => IconKind::PaintBucket,
    }
}

/// `tool`'s button description: its name and its shortcut key.
#[must_use]
pub fn tool_description(tool: Tool) -> String {
    format!("{} tool, shortcut {}", tool.label(), tool.shortcut_text())
}

/// The strip and its buttons, one per [`Tool::ALL`] entry, in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolsPanel {
    /// The `Role::Toolbar` strip.
    pub root: WidgetId,
    /// `(tool, its toggle button)`, in [`Tool::ALL`] order.
    pub buttons: [(Tool, WidgetId); Tool::ALL.len()],
    /// The foreground/background swatches and their two buttons (0.208.0).
    pub colours: ColourControls,
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

#[allow(clippy::cast_precision_loss)]
fn px(token: u32) -> f32 {
    token as f32
}

/// The strip's width: two buttons side by side plus `spacing.xxs` of
/// padding each side.
#[must_use]
pub fn tools_panel_width(scales: &Scales) -> f32 {
    TOOL_COLUMNS as f32 * widgets::icon_button_side(scales, TOOL_ICON_SIZE)
        + 2.0 * px(scales.spacing.xxs)
}

/// The strip's own style: a column, never shrinking below its content (so
/// a narrow window squeezes the canvas, never the tools), padded by
/// `spacing.xxs`, no gaps — each button carries its own `spacing.xxs` of
/// room round its icon.
fn strip_style(scales: &Scales) -> Style {
    let pad = length(px(scales.spacing.xxs));
    Style {
        flex_direction: FlexDirection::Column,
        flex_shrink: 0.0,
        padding: LayoutRect {
            left: pad,
            right: pad,
            top: pad,
            bottom: pad,
        },
        ..Default::default()
    }
}

/// One group's row: its buttons side by side from the left.
fn group_style() -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        flex_shrink: 0.0,
        size: Size {
            width: taffy::style_helpers::auto(),
            height: taffy::style_helpers::auto(),
        },
        ..Default::default()
    }
}

/// The style the label strip (`panel_strip`) uses: a column of text
/// buttons, padded and gapped by `spacing.xs`. (It began as the old
/// tools panel's style and kept it when the tools went to icons.)
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
    let root = tree.insert(parent, strip_style(scales), node, WidgetKind::Container)?;
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
    let mut slots = buttons.iter_mut();
    for (index, group) in TOOL_GROUPS.iter().enumerate() {
        if index > 0 {
            widgets::insert_separator(tree, root, scales)?;
        }
        let row = widgets::insert_container(tree, root, group_style())?;
        for &tool in *group {
            let id = widgets::insert_icon_toggle_button(
                tree,
                row,
                scales,
                tool.label(),
                tool_icon(tool),
                TOOL_ICON_SIZE,
                tool == selected,
            )?;
            widgets::set_button_description(tree, id, Some(&tool_description(tool)))?;
            if let Some(slot) = slots.next() {
                *slot = (tool, id);
            }
        }
    }
    widgets::insert_separator(tree, root, scales)?;
    let colours =
        insert_colour_controls(tree, root, scales, DEFAULT_FOREGROUND, DEFAULT_BACKGROUND)?;
    Ok(ToolsPanel {
        root,
        buttons,
        colours,
    })
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

/// Whether `id` is one of the panel's tool buttons.
#[must_use]
pub fn is_tools_panel_button(panel: &ToolsPanel, id: WidgetId) -> bool {
    panel.tool_for(id).is_some()
}

/// Points the hover tint at `hovered` (a button of this panel) and clears
/// it from every other one; `None` clears all. Returns whether anything
/// changed.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// if a button is missing or is not a button.
pub fn set_tools_panel_hover(
    tree: &mut WidgetTree<WidgetKind>,
    panel: &ToolsPanel,
    hovered: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    for (_, id) in panel.buttons {
        changed |= widgets::set_button_hovered(tree, id, hovered == Some(id))?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::{
        TOOL_COLUMNS, TOOL_GROUPS, TOOLS_PANEL_LABEL, is_tools_panel_button, selected_tool,
        set_tools_panel_hover, sync_tools_panel, tool_description, tool_icon, tools_panel_contains,
        tools_panel_width,
    };
    use crate::tool::Tool;
    use crate::workspace::{Workspace, build_workspace};
    use aurora_widgets::icons::IconKind;
    use aurora_widgets::widgets::{IconSize, WidgetKind};

    fn scales() -> aurora_theme::Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn bounds(ws: &Workspace, id: aurora_widgets::WidgetId) -> aurora_core::Rect {
        match ws.tree.bounds(id) {
            Some(bounds) => bounds,
            None => unreachable!("{id:?} is laid out"),
        }
    }

    /// The icon each tool must wear, written out independently of
    /// [`tool_icon`] so a mapped-to-the-wrong-tool slip fails.
    const EXPECTED_ICONS: [(Tool, IconKind); 16] = [
        (Tool::Move, IconKind::Move),
        (Tool::MarqueeSelect, IconKind::Marquee),
        (Tool::EllipticalMarquee, IconKind::EllipseMarquee),
        (Tool::Lasso, IconKind::Lasso),
        (Tool::PolygonalLasso, IconKind::PolygonalLasso),
        (Tool::MagicWand, IconKind::MagicWand),
        (Tool::Zoom, IconKind::ZoomIn),
        (Tool::Pan, IconKind::Hand),
        (Tool::Eyedropper, IconKind::Eyedropper),
        (Tool::Brush, IconKind::Brush),
        (Tool::Pencil, IconKind::Pencil),
        (Tool::Eraser, IconKind::Eraser),
        (Tool::CloneStamp, IconKind::CloneStamp),
        (Tool::Smudge, IconKind::Smudge),
        (Tool::Gradient, IconKind::Gradient),
        (Tool::PaintBucket, IconKind::PaintBucket),
    ];

    #[test]
    fn the_panel_is_a_labelled_vertical_toolbar_with_one_icon_toggle_per_tool() {
        let ws = build_workspace(&scales());
        let Some(node) = ws.tree.accessibility(ws.tools.root) else {
            unreachable!("built");
        };
        assert_eq!(node.role(), accesskit::Role::Toolbar);
        assert_eq!(node.label(), Some(TOOLS_PANEL_LABEL));
        assert_eq!(node.orientation(), Some(accesskit::Orientation::Vertical));
        for (tool, id) in ws.tools.buttons {
            let Some(node) = ws.tree.accessibility(id) else {
                unreachable!("built");
            };
            assert_eq!(node.role(), accesskit::Role::Button);
            // The full name is the accessible name, never drawn.
            assert_eq!(node.label(), Some(tool.label()));
            assert!(node.supports_action(accesskit::Action::Click));
            assert!(node.supports_action(accesskit::Action::Focus));
            let expected = if tool == Tool::default() {
                accesskit::Toggled::True
            } else {
                accesskit::Toggled::False
            };
            assert_eq!(node.toggled(), Some(expected), "{tool:?}");
            assert_eq!(ws.tools.tool_for(id), Some(tool));
            assert_eq!(ws.tools.button_for(tool), Some(id));
            assert!(tools_panel_contains(&ws.tree, &ws.tools, id));
            assert!(is_tools_panel_button(&ws.tools, id));
            // An icon button: the picture, no text.
            let Some(WidgetKind::Button(state)) = ws.tree.payload(id) else {
                unreachable!("a button");
            };
            assert_eq!(state.icon.map(|(_, size)| size), Some(IconSize::Large));
        }
        assert_eq!(ws.tools.tool_for(ws.canvas_area), None);
        assert!(!is_tools_panel_button(&ws.tools, ws.tools.root));
        assert_eq!(selected_tool(&ws.tree, &ws.tools), Some(Tool::default()));
    }

    /// Every tool wears its own icon, distinct from every other tool's.
    #[test]
    fn every_tool_button_wears_its_own_icon() {
        let ws = build_workspace(&scales());
        assert_eq!(EXPECTED_ICONS.len(), Tool::ALL.len());
        for (tool, icon) in EXPECTED_ICONS {
            assert_eq!(tool_icon(tool), icon, "{tool:?}");
            let Some(id) = ws.tools.button_for(tool) else {
                unreachable!("every tool has a button");
            };
            let Some(WidgetKind::Button(state)) = ws.tree.payload(id) else {
                unreachable!("a button");
            };
            assert_eq!(state.icon.map(|(kind, _)| kind), Some(icon), "{tool:?}");
        }
        let mut icons: Vec<_> = Tool::ALL.iter().map(|t| tool_icon(*t).name()).collect();
        icons.sort_unstable();
        icons.dedup();
        assert_eq!(icons.len(), Tool::ALL.len(), "no two tools share an icon");
    }

    /// A button's description is the tool's name and its shortcut letter.
    #[test]
    fn every_button_describes_its_tool_and_its_shortcut() {
        let ws = build_workspace(&scales());
        for (tool, id) in ws.tools.buttons {
            let Some(node) = ws.tree.accessibility(id) else {
                unreachable!("built");
            };
            let description = node.description().unwrap_or_default();
            assert_eq!(description, tool_description(tool));
            assert!(description.contains(tool.label()), "{description}");
            assert!(
                description.ends_with(&format!("shortcut {}", tool.shortcut_text())),
                "{description}"
            );
        }
        assert_eq!(tool_description(Tool::Brush), "Brush tool, shortcut B");
        assert_eq!(
            tool_description(Tool::Pencil),
            "Pencil tool, shortcut Shift+B"
        );
        assert_eq!(
            tool_description(Tool::CloneStamp),
            "Clone Stamp tool, shortcut S"
        );
        assert_eq!(tool_description(Tool::Smudge), "Smudge tool, shortcut R");
        assert_eq!(
            tool_description(Tool::Gradient),
            "Gradient tool, shortcut G"
        );
        assert_eq!(
            tool_description(Tool::PaintBucket),
            "Paint Bucket tool, shortcut Shift+G"
        );
        assert_eq!(
            tool_description(Tool::MarqueeSelect),
            "Marquee Select tool, shortcut M"
        );
        assert_eq!(
            tool_description(Tool::EllipticalMarquee),
            "Elliptical Marquee tool, shortcut Shift+M"
        );
        assert_eq!(tool_description(Tool::Lasso), "Lasso tool, shortcut L");
        assert_eq!(
            tool_description(Tool::PolygonalLasso),
            "Polygonal Lasso tool, shortcut Shift+L"
        );
        assert_eq!(
            tool_description(Tool::MagicWand),
            "Magic Wand tool, shortcut W"
        );
    }

    #[test]
    fn the_groups_partition_the_tools_in_order_with_a_rule_between_groups() {
        let flat: Vec<Tool> = TOOL_GROUPS.iter().flat_map(|g| g.iter().copied()).collect();
        assert_eq!(
            flat,
            Tool::ALL.to_vec(),
            "TOOL_GROUPS is Tool::ALL in order"
        );
        assert!(
            TOOL_GROUPS
                .iter()
                .all(|g| (1..=TOOL_COLUMNS).contains(&g.len()))
        );
        let ws = build_workspace(&scales());
        let children = ws.tree.children(ws.tools.root).unwrap_or_default();
        // group, rule, group, rule, group, rule, group
        assert_eq!(children.len(), 2 * TOOL_GROUPS.len() + 1);
        for (index, child) in children.iter().enumerate() {
            if index % 2 == 1 {
                let Some(node) = ws.tree.accessibility(*child) else {
                    unreachable!("built");
                };
                assert_eq!(node.role(), accesskit::Role::Splitter, "child {index}");
                assert!(!node.supports_action(accesskit::Action::Focus));
                assert!(matches!(
                    ws.tree.payload(*child),
                    Some(WidgetKind::MenuSeparator)
                ));
            } else {
                let group = TOOL_GROUPS.get(index / 2).copied().unwrap_or(&[]);
                let row = ws.tree.children(*child).unwrap_or_default();
                let tools: Vec<Tool> = row.iter().filter_map(|id| ws.tools.tool_for(*id)).collect();
                assert_eq!(tools, group.to_vec(), "group {}", index / 2);
            }
        }
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
                sync_tools_panel(&mut ws.tree, &ws.tools, Tool::PaintBucket),
                Ok(false)
            ),
            "syncing the same tool again changes nothing"
        );
    }

    /// Hover follows the pointer to exactly one button and clears.
    #[test]
    fn hover_tints_one_button_at_a_time() {
        let mut ws = build_workspace(&scales());
        let hovered = |ws: &Workspace| -> Vec<Tool> {
            ws.tools
                .buttons
                .iter()
                .filter(|(_, id)| {
                    matches!(ws.tree.payload(*id), Some(WidgetKind::Button(s)) if s.hovered)
                })
                .map(|(tool, _)| *tool)
                .collect()
        };
        let Some(zoom) = ws.tools.button_for(Tool::Zoom) else {
            unreachable!("built");
        };
        assert!(matches!(
            set_tools_panel_hover(&mut ws.tree, &ws.tools, Some(zoom)),
            Ok(true)
        ));
        assert_eq!(hovered(&ws), vec![Tool::Zoom]);
        assert!(matches!(
            set_tools_panel_hover(&mut ws.tree, &ws.tools, Some(zoom)),
            Ok(false)
        ));
        assert!(matches!(
            set_tools_panel_hover(&mut ws.tree, &ws.tools, None),
            Ok(true)
        ));
        assert!(hovered(&ws).is_empty());
    }

    /// `Tab` visits the seven buttons in `Tool::ALL` order, one stop
    /// each; the separators are never stops.
    #[test]
    fn tab_visits_each_button_once_in_tool_order_and_skips_the_rules() {
        let mut ws = build_workspace(&scales());
        let mut focus = aurora_widgets::FocusManager::new();
        let mut visited = Vec::new();
        for _ in 0..Tool::ALL.len() {
            let Some(id) = focus.focus_next(&mut ws.tree) else {
                unreachable!("a stop exists");
            };
            visited.push(id);
        }
        let expected: Vec<_> = ws.tools.buttons.iter().map(|(_, id)| *id).collect();
        assert_eq!(visited, expected);
    }

    /// 0.219.0: the strip's width follows the `size.icon_button` token
    /// (two buttons plus `spacing.xxs` each side), and the default is
    /// unchanged at 72.
    #[test]
    fn the_strip_width_follows_the_icon_button_token() {
        let mut scales = scales();
        assert!((tools_panel_width(&scales) - 72.0).abs() < f32::EPSILON);
        scales.size.icon_button = 36;
        assert!((tools_panel_width(&scales) - 80.0).abs() < f32::EPSILON);
    }

    /// 0.207.0: the strip is two 32 px buttons and `spacing.xxs` padding
    /// wide (72 px at the default), at scale 1 and under a 2x text engine
    /// alike, and narrower than the text-labelled column it replaces
    /// (rebuilt here from the same API the old panel used and measured with
    /// the real text engine).
    #[test]
    #[allow(clippy::too_many_lines)]
    fn the_strip_is_72_px_and_much_narrower_than_the_text_labelled_one() {
        let scales = scales();
        assert!((tools_panel_width(&scales) - 72.0).abs() < f32::EPSILON);
        for scale_factor in [1.0_f32, 2.0] {
            let mut ws = build_workspace(&scales);
            let Ok(mut engine) = aurora_text::TextEngine::new() else {
                unreachable!("the bundled font loads")
            };
            aurora_widgets::compute_text_layout(
                &mut ws.tree,
                1000.0,
                800.0,
                Some(aurora_widgets::TextMeasure {
                    engine: &mut engine,
                    scales: &scales,
                    scale_factor,
                }),
            );
            let strip = bounds(&ws, ws.tools.root);
            assert_eq!(strip.width, 72, "scale {scale_factor}");
            // The strip stretches to the window; its content is groups of
            // 32 px and 8 px rules under 4 px of padding: the Eraser ends
            // 4 + 8*32 + 7*8 = 316 px down (0.214.0 added the Lasso row and
            // the Magic Wand row, 0.220.0 the Pencil's: Brush and Pencil
            // share a row, the Eraser has the next one).
            let last = bounds(
                &ws,
                ws.tools.button_for(Tool::Eraser).unwrap_or(ws.tools.root),
            );
            assert_eq!(last.bottom() - strip.y, 316, "scale {scale_factor}");
            for (_, id) in ws.tools.buttons {
                let b = bounds(&ws, id);
                assert_eq!((b.width, b.height), (32, 32), "scale {scale_factor}");
            }
            // Two to a row, left cell first; the lone Eyedropper in the
            // left cell.
            let at = |tool| bounds(&ws, ws.tools.button_for(tool).unwrap_or(ws.tools.root));
            // 0.213.0: Move has a row of its own; the two marquees share
            // the next one, in the same two columns.
            assert!(at(Tool::MarqueeSelect).y > at(Tool::Move).y);
            assert_eq!(at(Tool::MarqueeSelect).y, at(Tool::EllipticalMarquee).y);
            assert_eq!(
                at(Tool::MarqueeSelect).x + 32,
                at(Tool::EllipticalMarquee).x
            );
            // 0.214.0: the two lassos share a row; the Magic Wand has one.
            assert_eq!(at(Tool::Lasso).y, at(Tool::PolygonalLasso).y);
            assert_eq!(at(Tool::Lasso).x + 32, at(Tool::PolygonalLasso).x);
            assert!(at(Tool::Lasso).y > at(Tool::EllipticalMarquee).y);
            assert!(at(Tool::MagicWand).y > at(Tool::Lasso).y);
            assert!(at(Tool::Zoom).y > at(Tool::MagicWand).y);
            assert_eq!(at(Tool::Eyedropper).x, at(Tool::Move).x);
            assert!(at(Tool::Zoom).y > at(Tool::Move).y);
            assert!(at(Tool::Brush).y > at(Tool::Eyedropper).y);
            // 0.220.0: the Pencil sits beside the Brush; the Eraser has
            // the row under them, in the left column.
            assert_eq!(at(Tool::Brush).y, at(Tool::Pencil).y);
            assert_eq!(at(Tool::Brush).x + 32, at(Tool::Pencil).x);
            assert!(at(Tool::Eraser).y > at(Tool::Brush).y);
            assert_eq!(at(Tool::Eraser).x, at(Tool::Brush).x);
            assert!(at(Tool::Gradient).y > at(Tool::Eraser).y);
            // 0.221.0: the Clone Stamp shares the Eraser's row.
            assert_eq!(at(Tool::CloneStamp).y, at(Tool::Eraser).y);
            assert_eq!(at(Tool::Eraser).x + 32, at(Tool::CloneStamp).x);
            // 0.222.0: the Smudge has a row of its own under them, in the
            // left column, above the fill row.
            assert!(at(Tool::Smudge).y > at(Tool::Eraser).y);
            assert_eq!(at(Tool::Smudge).x, at(Tool::Eraser).x);
            assert!(at(Tool::Gradient).y > at(Tool::Smudge).y);
            // 0.209.0: the fill row sits under the paint row, two to a
            // row, in the same two columns, inside the same 72 px.
            assert!(at(Tool::Gradient).y > at(Tool::Brush).y);
            assert_eq!(at(Tool::Gradient).y, at(Tool::PaintBucket).y);
            assert_eq!(at(Tool::Gradient).x, at(Tool::Brush).x);
            assert_eq!(at(Tool::PaintBucket).x, at(Tool::Pencil).x);
            let bucket = at(Tool::PaintBucket);
            assert_eq!(
                bucket.bottom() - strip.y,
                4 + 10 * 32 + 9 * 8,
                "scale {scale_factor}"
            );
            assert!(
                bucket.right() - strip.x <= 72 - 4,
                "inside the strip's padding"
            );
        }
        // The old strip, rebuilt: text toggle buttons in the label strip's
        // column style, measured.
        let (mut old, root) = aurora_widgets::widgets::new_tree(taffy::Style::default());
        let column = match aurora_widgets::widgets::insert_container(
            &mut old,
            root,
            super::tools_style(&scales),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        for tool in Tool::ALL {
            if let Err(err) = aurora_widgets::widgets::insert_toggle_button(
                &mut old,
                column,
                &scales,
                tool.label(),
                false,
            ) {
                unreachable!("{err:?}");
            }
        }
        let Ok(mut engine) = aurora_text::TextEngine::new() else {
            unreachable!("the bundled font loads")
        };
        aurora_widgets::compute_text_layout(
            &mut old,
            1000.0,
            800.0,
            Some(aurora_widgets::TextMeasure {
                engine: &mut engine,
                scales: &scales,
                scale_factor: 1.0,
            }),
        );
        let old_width = old.bounds(column).map_or(0, |b| b.width);
        eprintln!("the text-labelled strip measured {old_width} px wide");
        assert!(old_width > 100, "labels are measured: {old_width}");
        assert!(old_width > 72 + 40, "much wider than 72: {old_width}");
    }
}
