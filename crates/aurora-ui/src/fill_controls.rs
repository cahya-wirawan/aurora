//! The options bar's controls for the two fill tools (0.209.0): the
//! **Gradient** (a Linear/Radial type pair, a Reverse checkbox, an
//! Opacity readout and slider) and the **Paint Bucket** (a Tolerance
//! readout and slider, a Contiguous checkbox).
//!
//! They live inside the existing options strip ([`ToolControls::options`]),
//! after the radius readout and slider, as two group rows shown one at a
//! time: the active tool's group is `Display::Flex`, the other hidden both
//! by layout and from assistive technology (a hidden group hides its
//! whole subtree, so none of its widgets is a `Tab` stop or announced).
//! A fill tool also hides the radius readout and slider, which have
//! nothing to say for it. Because the groups are inside `options`,
//! [`crate::tool_controls_contains`] already routes their events to the
//! tool-control owner.
//!
//! **This module knows no tool parameters**, like [`crate::tool_controls`]:
//! `aurora-app` passes its settings as a [`FillView`] and applies the
//! outcomes of these widgets to its `ToolSettings`. Nothing here is
//! document state: no undo step, not persisted. The type pair is two text
//! toggle buttons that behave as a radio group (the app keeps exactly one
//! on). No new design tokens: sizes are the options bar's
//! (`size.options_control_width` for the sliders, `spacing.sm` gaps).

use accesskit::Toggled;
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::{TaffyZero as _, length};
use taffy::{Display, FlexDirection, Size, Style};

use crate::tool::Tool;
use crate::tool_controls::{
    OptionsItem, ToolControls, set_item_shown, set_node_hidden, set_options_item,
};

/// The Paint Bucket tolerance slider's range (8-bit-equivalent units).
pub const TOLERANCE_MIN: f64 = 0.0;
/// See [`TOLERANCE_MIN`].
pub const TOLERANCE_MAX: f64 = 255.0;
/// The Gradient opacity slider's range, in percent.
pub const OPACITY_MIN: f64 = 0.0;
/// See [`OPACITY_MIN`].
pub const OPACITY_MAX: f64 = 100.0;

/// The widgets of both groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FillControls {
    /// The Gradient group's row.
    pub gradient: WidgetId,
    /// The "Linear" type button.
    pub linear: WidgetId,
    /// The "Radial" type button.
    pub radial: WidgetId,
    /// The "Reverse" checkbox.
    pub reverse: WidgetId,
    /// "Opacity N %".
    pub opacity_readout: WidgetId,
    /// The opacity slider, 0 to 100.
    pub opacity: WidgetId,
    /// The Paint Bucket group's row.
    pub bucket: WidgetId,
    /// "Tolerance N".
    pub tolerance_readout: WidgetId,
    /// The tolerance slider, 0 to 255.
    pub tolerance: WidgetId,
    /// The "Contiguous" checkbox.
    pub contiguous: WidgetId,
}

/// The fill settings the controls mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FillView {
    /// Radial (`true`) or Linear (`false`).
    pub radial: bool,
    /// Reverse is on.
    pub reverse: bool,
    /// Gradient opacity, 0 to 100.
    pub opacity: u8,
    /// Paint Bucket tolerance, 0 to 255.
    pub tolerance: u8,
    /// Paint Bucket contiguous.
    pub contiguous: bool,
}

/// "Opacity 100 %".
#[must_use]
pub fn opacity_readout_text(percent: u8) -> String {
    format!("Opacity {percent} %")
}

/// "Tolerance 32".
#[must_use]
pub fn tolerance_readout_text(tolerance: u8) -> String {
    format!("Tolerance {tolerance}")
}

/// Whether `tool` is one of the two fill tools.
#[must_use]
pub const fn is_fill_tool(tool: Tool) -> bool {
    matches!(tool, Tool::Gradient | Tool::PaintBucket)
}

fn group_style(scales: &Scales) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let gap = length(scales.spacing.sm as f32);
    Style {
        display: Display::None,
        flex_direction: FlexDirection::Row,
        flex_grow: 1.0,
        align_items: Some(taffy::AlignItems::CENTER),
        gap: Size {
            width: gap,
            height: gap,
        },
        min_size: Size {
            width: taffy::Dimension::ZERO,
            height: taffy::Dimension::ZERO,
        },
        ..Default::default()
    }
}

/// Adds both groups as the last children of `options` (hidden).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `options` doesn't exist.
pub fn insert_fill_controls(
    tree: &mut WidgetTree<WidgetKind>,
    options: WidgetId,
    scales: &Scales,
) -> Result<FillControls, WidgetError> {
    let gradient = widgets::insert_container(tree, options, group_style(scales))?;
    let linear = widgets::insert_toggle_button(tree, gradient, scales, "Linear", true)?;
    let radial = widgets::insert_toggle_button(tree, gradient, scales, "Radial", false)?;
    let reverse = widgets::insert_checkbox(tree, gradient, scales, "Reverse")?;
    let opacity_readout =
        widgets::insert_label(tree, gradient, scales, opacity_readout_text(100))?;
    let opacity = widgets::insert_slider(
        tree,
        gradient,
        scales,
        "Gradient opacity",
        OPACITY_MAX,
        OPACITY_MIN,
        OPACITY_MAX,
    )?;
    set_options_item(tree, scales, opacity_readout, OptionsItem::Readout, true)?;
    set_options_item(tree, scales, opacity, OptionsItem::Slider, true)?;

    let bucket = widgets::insert_container(tree, options, group_style(scales))?;
    let tolerance_readout =
        widgets::insert_label(tree, bucket, scales, tolerance_readout_text(32))?;
    let tolerance = widgets::insert_slider(
        tree,
        bucket,
        scales,
        "Paint Bucket tolerance",
        32.0,
        TOLERANCE_MIN,
        TOLERANCE_MAX,
    )?;
    let contiguous = widgets::insert_checkbox(tree, bucket, scales, "Contiguous")?;
    widgets::set_checkbox_checked(tree, contiguous, Toggled::True)?;
    set_options_item(tree, scales, tolerance_readout, OptionsItem::Readout, true)?;
    set_options_item(tree, scales, tolerance, OptionsItem::Slider, true)?;

    set_node_hidden(tree, gradient, true)?;
    set_node_hidden(tree, bucket, true)?;
    Ok(FillControls {
        gradient,
        linear,
        radial,
        reverse,
        opacity_readout,
        opacity,
        bucket,
        tolerance_readout,
        tolerance,
        contiguous,
    })
}

fn sync_slider(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    value: f64,
    captured: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    if captured == Some(id) {
        return Ok(false);
    }
    let shown = match tree.payload(id) {
        Some(WidgetKind::Slider(state)) => state.value,
        Some(_) => return Err(WidgetError::WrongWidgetKind(id)),
        None => return Err(WidgetError::UnknownWidget(id)),
    };
    if shown.to_bits() == value.to_bits() {
        return Ok(false);
    }
    widgets::set_slider_value(tree, id, value)?;
    Ok(true)
}

fn sync_checkbox(
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

/// Shows the group of the active `tool` (none for any other tool),
/// hides the radius readout for a fill tool and shows it again for any
/// other, and mirrors `view` into the widgets. Never inserts or removes a
/// widget, so held ids and a pointer capture stay valid; `captured` is
/// the widget holding pointer capture (a slider mid-drag keeps its value).
/// Returns whether anything changed.
///
/// # Errors
///
/// Returns whatever the widget setters return for an id that is not the
/// kind `controls` says it is.
pub fn sync_fill_controls(
    tree: &mut WidgetTree<WidgetKind>,
    controls: &ToolControls,
    tool: Tool,
    view: FillView,
    captured: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    let fill = &controls.fill;
    let mut changed = false;
    for (group, shown) in [
        (fill.gradient, tool == Tool::Gradient),
        (fill.bucket, tool == Tool::PaintBucket),
    ] {
        changed |= set_item_shown(tree, group, shown)?;
        changed |= set_node_hidden(tree, group, !shown)?;
    }
    let fill_tool = is_fill_tool(tool);
    changed |= set_item_shown(tree, controls.readout, !fill_tool)?;
    changed |= set_node_hidden(tree, controls.readout, fill_tool)?;

    changed |= widgets::set_button_toggled(tree, fill.linear, !view.radial)?;
    changed |= widgets::set_button_toggled(tree, fill.radial, view.radial)?;
    changed |= sync_checkbox(tree, fill.reverse, view.reverse)?;
    changed |= widgets::set_label_text(
        tree,
        fill.opacity_readout,
        &opacity_readout_text(view.opacity),
    )?;
    changed |= sync_slider(tree, fill.opacity, f64::from(view.opacity), captured)?;
    changed |= widgets::set_label_text(
        tree,
        fill.tolerance_readout,
        &tolerance_readout_text(view.tolerance),
    )?;
    changed |= sync_slider(tree, fill.tolerance, f64::from(view.tolerance), captured)?;
    changed |= sync_checkbox(tree, fill.contiguous, view.contiguous)?;
    Ok(changed)
}

/// Whether the active tool's group is shown: `Some(tool)` for the group
/// currently displayed.
#[must_use]
pub fn shown_fill_group(tree: &WidgetTree<WidgetKind>, controls: &ToolControls) -> Option<Tool> {
    let shown = |id| tree.style(id).is_some_and(|s| s.display != Display::None);
    if shown(controls.fill.gradient) {
        Some(Tool::Gradient)
    } else if shown(controls.fill.bucket) {
        Some(Tool::PaintBucket)
    } else {
        None
    }
}
