//! The options bar's **Crop ratio** controls (0.236.0): a dropdown of
//! aspect-ratio presets and a "Portrait" checkbox, shown for the Crop tool
//! only.
//!
//! One group row inside the options strip, built like the selection-mode
//! group ([`crate::selection_mode_controls`]): hidden by layout and from
//! assistive technology unless the Crop tool is active, nothing in it
//! shrinks (the dropdown has a fixed floor-width, the checkbox its measured
//! text, see [`crate::tool_controls`]'s 0.228.3 rules), and the bar clips
//! an end that does not fit.
//!
//! **This module owns no setting.** `aurora-app` passes the ratio and the
//! orientation in force ([`sync_crop_controls`] re-mirrors them after
//! whatever the widgets did, so the controls can never disagree with the
//! settings) and applies the dropdown's committed option or the checkbox
//! toggle. Nothing here is document state. No new design tokens
//! (`spacing.sm` gaps, the slider floor for the dropdown's width).

use accesskit::Toggled;
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{Display, FlexDirection, Size, Style};

use crate::tool::Tool;
use crate::tool_controls::{
    OptionsItem, ToolControls, set_item_shown, set_node_hidden, set_options_item, slider_floor,
};

/// The dropdown's accessible name.
pub const CROP_RATIO_LABEL: &str = "Crop ratio";

/// The checkbox's label and accessible name.
pub const CROP_PORTRAIT_LABEL: &str = "Portrait";

/// The group's accessible name.
pub const CROP_GROUP_LABEL: &str = "Crop options";

/// The Crop tool's aspect-ratio constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CropRatio {
    /// No constraint (the default).
    #[default]
    Free,
    /// The canvas's own width : height.
    Original,
    /// 1:1.
    Square,
    /// 3:2.
    ThreeTwo,
    /// 4:3.
    FourThree,
    /// 5:4.
    FiveFour,
    /// 16:9.
    SixteenNine,
    /// 16:10.
    SixteenTen,
}

impl CropRatio {
    /// Every choice, in the dropdown's order.
    pub const ALL: [Self; 8] = [
        Self::Free,
        Self::Original,
        Self::Square,
        Self::ThreeTwo,
        Self::FourThree,
        Self::FiveFour,
        Self::SixteenNine,
        Self::SixteenTen,
    ];

    /// The dropdown option's text.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Original => "Original",
            Self::Square => "1:1",
            Self::ThreeTwo => "3:2",
            Self::FourThree => "4:3",
            Self::FiveFour => "5:4",
            Self::SixteenNine => "16:9",
            Self::SixteenTen => "16:10",
        }
    }

    /// The preset's landscape `(width, height)`; `None` for Free and
    /// Original (the canvas decides).
    #[must_use]
    pub const fn fixed(self) -> Option<(u32, u32)> {
        match self {
            Self::Free | Self::Original => None,
            Self::Square => Some((1, 1)),
            Self::ThreeTwo => Some((3, 2)),
            Self::FourThree => Some((4, 3)),
            Self::FiveFour => Some((5, 4)),
            Self::SixteenNine => Some((16, 9)),
            Self::SixteenTen => Some((16, 10)),
        }
    }

    /// The dropdown index of the choice.
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|&candidate| candidate == self)
            .unwrap_or(0)
    }

    /// The choice at dropdown `index`.
    #[must_use]
    pub fn at(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

/// The widgets of the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CropControls {
    /// The group's row.
    pub group: WidgetId,
    /// The ratio dropdown ([`CROP_RATIO_LABEL`]).
    pub ratio: WidgetId,
    /// The "Portrait" checkbox.
    pub portrait: WidgetId,
}

/// Whether `tool` shows the group.
#[must_use]
pub const fn has_crop_controls(tool: Tool) -> bool {
    matches!(tool, Tool::Crop)
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
pub fn insert_crop_controls(
    tree: &mut WidgetTree<WidgetKind>,
    options: WidgetId,
    scales: &Scales,
) -> Result<CropControls, WidgetError> {
    let group = widgets::insert_container(tree, options, group_style(scales))?;
    let mut node = accesskit::Node::new(accesskit::Role::Group);
    node.set_label(CROP_GROUP_LABEL);
    tree.set_accessibility(group, node)?;
    let ratio = widgets::insert_dropdown(
        tree,
        group,
        scales,
        CROP_RATIO_LABEL,
        CropRatio::ALL
            .iter()
            .map(|choice| choice.label().to_owned())
            .collect(),
        Some(CropRatio::Free.index()),
    )?;
    // The dropdown's default style is "100 % of the parent": in a row that
    // is nothing. A fixed width (the slider floor), never shrunk.
    let mut style = tree
        .style(ratio)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(ratio))?;
    #[allow(clippy::cast_precision_loss)]
    let width = length(slider_floor(scales) as f32);
    style.size.width = width;
    style.min_size.width = width;
    style.flex_shrink = 0.0;
    tree.set_style(ratio, style)?;
    let portrait = widgets::insert_checkbox(tree, group, scales, CROP_PORTRAIT_LABEL)?;
    set_options_item(tree, scales, portrait, OptionsItem::Check, true)?;
    set_node_hidden(tree, group, true)?;
    Ok(CropControls {
        group,
        ratio,
        portrait,
    })
}

/// Shows the group for the Crop tool (hidden for any other, with an open
/// list closed), and mirrors `ratio` and `portrait`. Never inserts or
/// removes a widget, so held ids stay valid. Returns whether anything
/// changed.
///
/// # Errors
///
/// Returns whatever the widget setters return for an id that is not the
/// kind `controls` says it is.
pub fn sync_crop_controls(
    tree: &mut WidgetTree<WidgetKind>,
    controls: &ToolControls,
    tool: Tool,
    ratio: CropRatio,
    portrait: bool,
) -> Result<bool, WidgetError> {
    let crop = &controls.crop;
    let shown = has_crop_controls(tool);
    let mut changed = set_item_shown(tree, crop.group, shown)?;
    changed |= set_node_hidden(tree, crop.group, !shown)?;
    let (selected, open) = {
        let state = widgets::dropdown_state(tree, crop.ratio)?;
        (state.selected(), state.is_open())
    };
    if !shown && open {
        widgets::set_dropdown_open(tree, crop.ratio, false)?;
        changed = true;
    }
    if selected != Some(ratio.index()) {
        widgets::set_dropdown_selected(tree, crop.ratio, Some(ratio.index()))?;
        changed = true;
    }
    let want = if portrait {
        Toggled::True
    } else {
        Toggled::False
    };
    match tree.payload(crop.portrait) {
        Some(WidgetKind::Checkbox(state)) if state.checked == want => {}
        Some(WidgetKind::Checkbox(_)) => {
            widgets::set_checkbox_checked(tree, crop.portrait, want)?;
            changed = true;
        }
        Some(_) => return Err(WidgetError::WrongWidgetKind(crop.portrait)),
        None => return Err(WidgetError::UnknownWidget(crop.portrait)),
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::{
        CROP_PORTRAIT_LABEL, CROP_RATIO_LABEL, CropRatio, has_crop_controls, sync_crop_controls,
    };
    use crate::tool::Tool;
    use crate::tool_controls::{ToolControls, insert_tool_controls, tool_controls_contains};
    use crate::workspace::build_workspace;
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
        (ws, controls)
    }

    fn sync(
        ws: &mut crate::Workspace,
        c: &ToolControls,
        tool: Tool,
        ratio: CropRatio,
        portrait: bool,
    ) -> bool {
        let changed = match sync_crop_controls(&mut ws.tree, c, tool, ratio, portrait) {
            Ok(changed) => changed,
            Err(err) => unreachable!("{err:?}"),
        };
        ws.tree.compute_layout(1600.0, 900.0);
        changed
    }

    fn shown(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
        tree.style(id)
            .is_some_and(|style| style.display != Display::None)
    }

    #[test]
    fn the_choices_are_the_eight_presets_in_order_with_stable_indices() {
        let labels: Vec<_> = CropRatio::ALL.iter().map(|c| c.label()).collect();
        assert_eq!(
            labels,
            [
                "Free", "Original", "1:1", "3:2", "4:3", "5:4", "16:9", "16:10"
            ]
        );
        for (index, choice) in CropRatio::ALL.into_iter().enumerate() {
            assert_eq!(choice.index(), index);
            assert_eq!(CropRatio::at(index), Some(choice));
        }
        assert_eq!(CropRatio::at(8), None);
        assert_eq!(CropRatio::default(), CropRatio::Free);
        assert_eq!(CropRatio::SixteenTen.fixed(), Some((16, 10)));
        assert_eq!(CropRatio::Free.fixed(), None);
        assert_eq!(CropRatio::Original.fixed(), None);
    }

    #[test]
    fn the_group_is_for_the_crop_tool_only_and_hidden_from_at_otherwise() {
        let (mut ws, c) = built();
        for tool in Tool::ALL {
            sync(&mut ws, &c, tool, CropRatio::Free, false);
            let crop = tool == Tool::Crop;
            assert_eq!(has_crop_controls(tool), crop, "{tool:?}");
            assert_eq!(shown(&ws.tree, c.crop.group), crop, "{tool:?}");
            assert_eq!(
                ws.tree
                    .accessibility(c.crop.group)
                    .is_some_and(accesskit::Node::is_hidden),
                !crop,
                "{tool:?}"
            );
        }
    }

    #[test]
    fn the_widgets_are_named_mirrored_and_have_size_when_shown() {
        let (mut ws, c) = built();
        sync(&mut ws, &c, Tool::Crop, CropRatio::ThreeTwo, true);
        assert_eq!(
            ws.tree.accessibility(c.crop.ratio).and_then(|n| n.label()),
            Some(CROP_RATIO_LABEL)
        );
        assert_eq!(
            ws.tree
                .accessibility(c.crop.portrait)
                .and_then(|n| n.label()),
            Some(CROP_PORTRAIT_LABEL)
        );
        let selected = match ws.tree.payload(c.crop.ratio) {
            Some(WidgetKind::Dropdown(state)) => state.selected(),
            _ => unreachable!("a dropdown"),
        };
        assert_eq!(selected, Some(CropRatio::ThreeTwo.index()));
        let checked = |ws: &crate::Workspace| {
            matches!(
                ws.tree.payload(c.crop.portrait),
                Some(WidgetKind::Checkbox(s)) if s.checked == accesskit::Toggled::True
            )
        };
        assert!(checked(&ws));
        for id in [c.crop.ratio, c.crop.portrait] {
            assert!(tool_controls_contains(&ws.tree, &c, id));
            let rect = ws.tree.bounds(id);
            assert!(
                rect.is_some_and(|r| r.width > 0 && r.height > 0),
                "{id:?}: {rect:?}"
            );
            assert_eq!(
                ws.tree.style(id).map(|s| s.flex_shrink),
                Some(0.0),
                "{id:?} never shrinks"
            );
        }
        // A re-sync after the widgets changed brings them back.
        sync(&mut ws, &c, Tool::Crop, CropRatio::Free, false);
        assert!(!checked(&ws));
        assert!(!sync(&mut ws, &c, Tool::Crop, CropRatio::Free, false));
    }

    /// The 0.228.3 sizing rules: neither item shrinks in a narrow window,
    /// the dropdown is the slider floor wide (a token-derived width) at any
    /// bar width, and both keep a hittable extent.
    #[test]
    fn the_dropdown_keeps_its_floor_width_and_the_checkbox_its_text_in_a_narrow_bar() {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        let floor = match Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales.size.options_control_width / 2,
            Err(err) => unreachable!("{err:?}"),
        };
        let (mut ws, c) = built();
        sync(&mut ws, &c, Tool::Crop, CropRatio::Free, false);
        let widths = |ws: &crate::Workspace| {
            [c.crop.ratio, c.crop.portrait]
                .map(|id| ws.tree.bounds(id).map_or(0, |bounds| bounds.width))
        };
        let wide = widths(&ws);
        assert_eq!(wide[0], floor, "the dropdown is the slider floor wide");
        assert!(wide[1] > 0);
        for window in [1000.0_f32, 700.0] {
            ws.tree.compute_layout(window, 900.0);
            assert_eq!(widths(&ws), wide, "{window}: nothing shrinks");
        }
    }

    #[test]
    fn leaving_the_tool_closes_an_open_list() {
        let (mut ws, c) = built();
        sync(&mut ws, &c, Tool::Crop, CropRatio::Free, false);
        if let Err(err) =
            aurora_widgets::widgets::set_dropdown_open(&mut ws.tree, c.crop.ratio, true)
        {
            unreachable!("{err:?}");
        }
        sync(&mut ws, &c, Tool::Brush, CropRatio::Free, false);
        assert!(
            aurora_widgets::widgets::dropdown_state(&ws.tree, c.crop.ratio)
                .is_ok_and(|state| !state.is_open())
        );
    }
}
