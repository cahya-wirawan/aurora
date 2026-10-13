//! The options bar's **Range** and **Mode** controls for the toning tools
//! (0.224.0): Shadows / Midtones / Highlights for the Dodge and the Burn,
//! Desaturate / Saturate for the Sponge.
//!
//! Two groups of checkboxes inside the options strip, built like the
//! selection-mode group ([`crate::selection_mode_controls`]) and kept a
//! radio group the same way: whatever a checkbox did,
//! [`sync_tone_controls`] re-mirrors the one choice in force, so exactly one
//! box of a group is ever checked. The Range group is shown for the Dodge and
//! Burn (which share it), the Mode group for the Sponge, and each is hidden
//! by layout and from assistive technology otherwise.
//!
//! **This module owns no setting**: `aurora-app` passes the choices in force
//! and applies the checkbox that changed. Nothing here is document state.
//! No new design tokens (`spacing.sm` gaps).

use accesskit::Toggled;
use aurora_brush::{SpongeMode, ToneRange};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{Display, FlexDirection, Size, Style};

use crate::tool::Tool;
use crate::tool_controls::{ToolControls, set_item_shown, set_node_hidden};

/// The Range group's accessible name.
pub const TONE_RANGE_LABEL: &str = "Range";

/// The Mode group's accessible name.
pub const SPONGE_MODE_LABEL: &str = "Mode";

/// The widgets of both groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToneControls {
    /// The Range group's row (Dodge and Burn).
    pub range_group: WidgetId,
    /// "Shadows".
    pub shadows: WidgetId,
    /// "Midtones".
    pub midtones: WidgetId,
    /// "Highlights".
    pub highlights: WidgetId,
    /// The Mode group's row (Sponge).
    pub mode_group: WidgetId,
    /// "Desaturate".
    pub desaturate: WidgetId,
    /// "Saturate".
    pub saturate: WidgetId,
}

impl ToneControls {
    /// The checkbox of `range`.
    #[must_use]
    pub const fn range_checkbox(&self, range: ToneRange) -> WidgetId {
        match range {
            ToneRange::Shadows => self.shadows,
            ToneRange::Midtones => self.midtones,
            ToneRange::Highlights => self.highlights,
        }
    }

    /// The range whose checkbox is `id`, if it is one of the three.
    #[must_use]
    pub fn range_of(&self, id: WidgetId) -> Option<ToneRange> {
        ToneRange::ALL
            .into_iter()
            .find(|&range| self.range_checkbox(range) == id)
    }

    /// The checkbox of `mode`.
    #[must_use]
    pub const fn mode_checkbox(&self, mode: SpongeMode) -> WidgetId {
        match mode {
            SpongeMode::Desaturate => self.desaturate,
            SpongeMode::Saturate => self.saturate,
        }
    }

    /// The Sponge mode whose checkbox is `id`, if it is one of the two.
    #[must_use]
    pub fn mode_of(&self, id: WidgetId) -> Option<SpongeMode> {
        SpongeMode::ALL
            .into_iter()
            .find(|&mode| self.mode_checkbox(mode) == id)
    }
}

/// The checkbox label of `range`.
#[must_use]
pub const fn tone_range_label(range: ToneRange) -> &'static str {
    match range {
        ToneRange::Shadows => "Shadows",
        ToneRange::Midtones => "Midtones",
        ToneRange::Highlights => "Highlights",
    }
}

/// The checkbox label of `mode`.
#[must_use]
pub const fn sponge_mode_label(mode: SpongeMode) -> &'static str {
    match mode {
        SpongeMode::Desaturate => "Desaturate",
        SpongeMode::Saturate => "Saturate",
    }
}

/// Whether `tool` shows the Range group.
#[must_use]
pub const fn has_tone_range(tool: Tool) -> bool {
    matches!(tool, Tool::Dodge | Tool::Burn)
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

fn insert_group(
    tree: &mut WidgetTree<WidgetKind>,
    options: WidgetId,
    scales: &Scales,
    name: &str,
) -> Result<WidgetId, WidgetError> {
    let group = widgets::insert_container(tree, options, group_style(scales))?;
    let mut node = accesskit::Node::new(accesskit::Role::Group);
    node.set_label(name);
    tree.set_accessibility(group, node)?;
    Ok(group)
}

/// Adds both groups as the last children of `options` (hidden).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `options` doesn't exist.
pub fn insert_tone_controls(
    tree: &mut WidgetTree<WidgetKind>,
    options: WidgetId,
    scales: &Scales,
) -> Result<ToneControls, WidgetError> {
    let range_group = insert_group(tree, options, scales, TONE_RANGE_LABEL)?;
    let mut ranges = [range_group; 3];
    for (slot, range) in ranges.iter_mut().zip(ToneRange::ALL) {
        *slot = widgets::insert_checkbox(tree, range_group, scales, tone_range_label(range))?;
    }
    let [shadows, midtones, highlights] = ranges;
    widgets::set_checkbox_checked(tree, midtones, Toggled::True)?;

    let mode_group = insert_group(tree, options, scales, SPONGE_MODE_LABEL)?;
    let mut modes = [mode_group; 2];
    for (slot, mode) in modes.iter_mut().zip(SpongeMode::ALL) {
        *slot = widgets::insert_checkbox(tree, mode_group, scales, sponge_mode_label(mode))?;
    }
    let [desaturate, saturate] = modes;
    widgets::set_checkbox_checked(tree, desaturate, Toggled::True)?;

    set_node_hidden(tree, range_group, true)?;
    set_node_hidden(tree, mode_group, true)?;
    Ok(ToneControls {
        range_group,
        shadows,
        midtones,
        highlights,
        mode_group,
        desaturate,
        saturate,
    })
}

fn sync_radio(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    on: bool,
) -> Result<bool, WidgetError> {
    let want = if on { Toggled::True } else { Toggled::False };
    match tree.payload(id) {
        Some(WidgetKind::Checkbox(state)) if state.checked == want => Ok(false),
        Some(WidgetKind::Checkbox(_)) => {
            widgets::set_checkbox_checked(tree, id, want)?;
            Ok(true)
        }
        Some(_) => Err(WidgetError::WrongWidgetKind(id)),
        None => Err(WidgetError::UnknownWidget(id)),
    }
}

/// Shows the Range group for the Dodge and Burn and the Mode group for the
/// Sponge (each hidden for any other tool), and checks exactly the
/// checkbox of `range` and of `mode`. Never inserts or removes a widget,
/// so held ids stay valid. Returns whether anything changed.
///
/// # Errors
///
/// Returns whatever the widget setters return for an id that is not the
/// kind `controls` says it is.
pub fn sync_tone_controls(
    tree: &mut WidgetTree<WidgetKind>,
    controls: &ToolControls,
    tool: Tool,
    range: ToneRange,
    mode: SpongeMode,
) -> Result<bool, WidgetError> {
    let tone = &controls.tone;
    let mut changed = false;
    for (group, shown) in [
        (tone.range_group, has_tone_range(tool)),
        (tone.mode_group, tool == Tool::Sponge),
    ] {
        changed |= set_item_shown(tree, group, shown)?;
        changed |= set_node_hidden(tree, group, !shown)?;
    }
    for candidate in ToneRange::ALL {
        changed |= sync_radio(tree, tone.range_checkbox(candidate), candidate == range)?;
    }
    for candidate in SpongeMode::ALL {
        changed |= sync_radio(tree, tone.mode_checkbox(candidate), candidate == mode)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::{
        SPONGE_MODE_LABEL, TONE_RANGE_LABEL, has_tone_range, sponge_mode_label, sync_tone_controls,
        tone_range_label,
    };
    use crate::tool::Tool;
    use crate::tool_controls::{ToolControls, insert_tool_controls, tool_controls_contains};
    use crate::workspace::build_workspace;
    use aurora_brush::{SpongeMode, ToneRange};
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

    fn sync(
        ws: &mut crate::Workspace,
        c: &ToolControls,
        tool: Tool,
        range: ToneRange,
        mode: SpongeMode,
    ) {
        if let Err(err) = sync_tone_controls(&mut ws.tree, c, tool, range, mode) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);
    }

    fn shown(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
        tree.style(id).is_some_and(|s| s.display != Display::None)
    }

    fn hidden_to_at(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
        tree.accessibility(id)
            .is_some_and(accesskit::Node::is_hidden)
    }

    fn checked(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
        matches!(
            tree.payload(id),
            Some(WidgetKind::Checkbox(state)) if state.checked == accesskit::Toggled::True
        )
    }

    #[test]
    fn the_range_group_is_for_dodge_and_burn_and_the_mode_group_for_the_sponge() {
        let (mut ws, c) = built();
        for tool in Tool::ALL {
            sync(
                &mut ws,
                &c,
                tool,
                ToneRange::Midtones,
                SpongeMode::Desaturate,
            );
            let range = matches!(tool, Tool::Dodge | Tool::Burn);
            let mode = tool == Tool::Sponge;
            assert_eq!(has_tone_range(tool), range, "{tool:?}");
            assert_eq!(shown(&ws.tree, c.tone.range_group), range, "{tool:?}");
            assert_eq!(shown(&ws.tree, c.tone.mode_group), mode, "{tool:?}");
            assert_eq!(
                hidden_to_at(&ws.tree, c.tone.range_group),
                !range,
                "{tool:?}"
            );
            assert_eq!(hidden_to_at(&ws.tree, c.tone.mode_group), !mode, "{tool:?}");
        }
    }

    #[test]
    fn exactly_the_choice_in_force_is_checked_in_each_group_and_shown_boxes_have_size() {
        let (mut ws, c) = built();
        for range in ToneRange::ALL {
            for mode in SpongeMode::ALL {
                sync(&mut ws, &c, Tool::Dodge, range, mode);
                for other in ToneRange::ALL {
                    assert_eq!(
                        checked(&ws.tree, c.tone.range_checkbox(other)),
                        other == range,
                        "{range:?} / {other:?}"
                    );
                }
                for other in SpongeMode::ALL {
                    assert_eq!(
                        checked(&ws.tree, c.tone.mode_checkbox(other)),
                        other == mode,
                        "{mode:?} / {other:?}"
                    );
                }
            }
        }
        sync(
            &mut ws,
            &c,
            Tool::Burn,
            ToneRange::Shadows,
            SpongeMode::Saturate,
        );
        for range in ToneRange::ALL {
            let id = c.tone.range_checkbox(range);
            let rect = ws.tree.bounds(id);
            assert!(
                rect.is_some_and(|r| r.width > 0 && r.height > 0),
                "{range:?} has size: {rect:?}"
            );
            assert!(tool_controls_contains(&ws.tree, &c, id));
            assert_eq!(c.tone.range_of(id), Some(range));
            assert_eq!(c.tone.mode_of(id), None);
        }
        sync(
            &mut ws,
            &c,
            Tool::Sponge,
            ToneRange::Shadows,
            SpongeMode::Saturate,
        );
        for mode in SpongeMode::ALL {
            let id = c.tone.mode_checkbox(mode);
            let rect = ws.tree.bounds(id);
            assert!(
                rect.is_some_and(|r| r.width > 0 && r.height > 0),
                "{mode:?} has size: {rect:?}"
            );
            assert_eq!(c.tone.mode_of(id), Some(mode));
            assert_eq!(c.tone.range_of(id), None);
        }
        assert_eq!(c.tone.range_of(c.radius), None);
        assert_eq!(c.tone.mode_of(c.radius), None);
    }

    #[test]
    fn labels_and_group_names_are_as_documented() {
        assert_eq!(tone_range_label(ToneRange::Shadows), "Shadows");
        assert_eq!(tone_range_label(ToneRange::Midtones), "Midtones");
        assert_eq!(tone_range_label(ToneRange::Highlights), "Highlights");
        assert_eq!(sponge_mode_label(SpongeMode::Desaturate), "Desaturate");
        assert_eq!(sponge_mode_label(SpongeMode::Saturate), "Saturate");
        assert_eq!(TONE_RANGE_LABEL, "Range");
        assert_eq!(SPONGE_MODE_LABEL, "Mode");
        let (ws, c) = built();
        for (id, name) in [
            (c.tone.range_group, TONE_RANGE_LABEL),
            (c.tone.mode_group, SPONGE_MODE_LABEL),
        ] {
            let node = ws.tree.accessibility(id);
            assert_eq!(
                node.map(accesskit::Node::role),
                Some(accesskit::Role::Group)
            );
            assert_eq!(node.and_then(accesskit::Node::label), Some(name));
        }
        // Midtones and Desaturate are the defaults the controls start on.
        assert!(checked(&ws.tree, c.tone.midtones));
        assert!(checked(&ws.tree, c.tone.desaturate));
    }
}
