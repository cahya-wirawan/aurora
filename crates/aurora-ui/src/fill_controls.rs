//! The options bar's controls for the two fill tools (0.209.0): the
//! **Gradient** (a Linear/Radial type pair, a Reverse checkbox, an
//! Opacity readout and slider) and the **Paint Bucket** (a Tolerance
//! readout and slider, a Contiguous checkbox), and (0.222.0) the **Smudge**'s
//! Strength readout and slider, built the way the Gradient's opacity is.
//! The Smudge keeps the radius readout and slider (it has a radius); only
//! the fill tools hide them.
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
//! document state: no undo step, not persisted. The type is one checkbox,
//! "Radial" (a toggle pair is taller than the bar's row and would make the
//! canvas jump on a tool switch). No new design tokens: sizes are the options bar's
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

/// The Smudge strength slider's range, in percent (0.222.0).
pub const STRENGTH_MIN: f64 = 1.0;
/// See [`STRENGTH_MIN`].
pub const STRENGTH_MAX: f64 = 100.0;

/// The widgets of all three groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FillControls {
    /// The Gradient group's row.
    pub gradient: WidgetId,
    /// The "Radial" checkbox: unchecked is Linear, checked Radial.
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
    /// The Smudge group's row (0.222.0).
    pub smudge: WidgetId,
    /// "Strength N %".
    pub strength_readout: WidgetId,
    /// The strength slider, 1 to 100.
    pub strength: WidgetId,
    /// The paint group's row (0.228.0): Opacity and Flow for the Brush,
    /// Pencil (Opacity only) and Clone Stamp.
    pub paint: WidgetId,
    /// "Opacity N %" of the paint group.
    pub paint_opacity_readout: WidgetId,
    /// The paint Opacity slider, 1 to 100.
    pub paint_opacity: WidgetId,
    /// "Flow N %" of the paint group (hidden for the Pencil).
    pub paint_flow_readout: WidgetId,
    /// The paint Flow slider, 1 to 100.
    pub paint_flow: WidgetId,
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
    /// Smudge strength, 1 to 100 (0.222.0).
    pub strength: u8,
    /// Blur strength, 1 to 100 (0.223.0).
    pub blur_strength: u8,
    /// Sharpen strength, 1 to 100 (0.223.0).
    pub sharpen_strength: u8,
    /// Dodge exposure, 1 to 100 (0.224.0).
    pub dodge_exposure: u8,
    /// Burn exposure, 1 to 100 (0.224.0).
    pub burn_exposure: u8,
    /// Sponge flow, 1 to 100 (0.224.0).
    pub sponge_flow: u8,
    /// Brush opacity, 1 to 100 (0.228.0).
    pub brush_opacity: u8,
    /// Brush flow, 1 to 100 (0.228.0).
    pub brush_flow: u8,
    /// Pencil opacity, 1 to 100 (0.228.0).
    pub pencil_opacity: u8,
    /// Clone Stamp opacity, 1 to 100 (0.228.0).
    pub clone_opacity: u8,
    /// Clone Stamp flow, 1 to 100 (0.228.0).
    pub clone_flow: u8,
}

/// "Opacity 100 %".
#[must_use]
pub fn opacity_readout_text(percent: u8) -> String {
    format!("Opacity {percent} %")
}

/// "Flow 100 %" (0.228.0).
#[must_use]
pub fn flow_readout_text(percent: u8) -> String {
    format!("Flow {percent} %")
}

/// Whether `tool` shows the paint group's Opacity (0.228.0): the Brush,
/// the Pencil and the Clone Stamp.
#[must_use]
pub const fn has_paint_opacity(tool: Tool) -> bool {
    matches!(tool, Tool::Brush | Tool::Pencil | Tool::CloneStamp)
}

/// Whether `tool` also shows the paint group's Flow (0.228.0): the Brush
/// and the Clone Stamp, not the Pencil.
#[must_use]
pub const fn has_paint_flow(tool: Tool) -> bool {
    matches!(tool, Tool::Brush | Tool::CloneStamp)
}

/// "Strength 50 %" (0.222.0).
#[must_use]
pub fn strength_readout_text(percent: u8) -> String {
    format!("Strength {percent} %")
}

/// What the shared slider is called for `tool` (0.224.0): "Exposure" for
/// the Dodge and Burn, "Flow" for the Sponge, "Strength" for the rest. It
/// is both the readout's word and the slider's accessible name.
#[must_use]
pub const fn strength_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Dodge | Tool::Burn => "Exposure",
        Tool::Sponge => "Flow",
        _ => "Strength",
    }
}

/// "Exposure 50 %", "Flow 50 %" or "Strength 50 %": the readout for `tool`.
#[must_use]
pub fn strength_readout_for(tool: Tool, percent: u8) -> String {
    format!("{} {percent} %", strength_name(tool))
}

/// "Tolerance 32".
#[must_use]
pub fn tolerance_readout_text(tolerance: u8) -> String {
    format!("Tolerance {tolerance}")
}

/// Whether `tool` shows a fill options group instead of the radius
/// readout: the two fill tools and the Magic Wand (0.214.0), which
/// shares the Paint Bucket's group.
#[must_use]
pub const fn is_fill_tool(tool: Tool) -> bool {
    matches!(tool, Tool::Gradient | Tool::PaintBucket | Tool::MagicWand)
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
    // The type is one checkbox (a row tall, like every other options-bar
    // control): a text or icon toggle pair is taller than the bar's row
    // (a measured text toggle is 45 px, a 32 px icon button 48 px with the
    // bar's padding against 37), which would make the canvas jump on a
    // tool switch.
    let radial = widgets::insert_checkbox(tree, gradient, scales, "Radial")?;
    let reverse = widgets::insert_checkbox(tree, gradient, scales, "Reverse")?;
    let opacity_readout = widgets::insert_label(tree, gradient, scales, opacity_readout_text(100))?;
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

    let smudge = widgets::insert_container(tree, options, group_style(scales))?;
    let strength_readout = widgets::insert_label(tree, smudge, scales, strength_readout_text(50))?;
    let strength = widgets::insert_slider(
        tree,
        smudge,
        scales,
        "Strength",
        50.0,
        STRENGTH_MIN,
        STRENGTH_MAX,
    )?;
    set_options_item(tree, scales, strength_readout, OptionsItem::Readout, true)?;
    set_options_item(tree, scales, strength, OptionsItem::Slider, true)?;

    // The Brush, Pencil and Clone Stamp (0.228.0): Opacity, and Flow for the
    // two that have one.
    let paint = widgets::insert_container(tree, options, group_style(scales))?;
    let paint_opacity_readout =
        widgets::insert_label(tree, paint, scales, opacity_readout_text(100))?;
    let paint_opacity = widgets::insert_slider(
        tree,
        paint,
        scales,
        "Opacity",
        100.0,
        STRENGTH_MIN,
        STRENGTH_MAX,
    )?;
    let paint_flow_readout = widgets::insert_label(tree, paint, scales, flow_readout_text(100))?;
    let paint_flow = widgets::insert_slider(
        tree,
        paint,
        scales,
        "Flow",
        100.0,
        STRENGTH_MIN,
        STRENGTH_MAX,
    )?;
    for (id, item) in [
        (paint_opacity_readout, OptionsItem::Readout),
        (paint_opacity, OptionsItem::Slider),
        (paint_flow_readout, OptionsItem::Readout),
        (paint_flow, OptionsItem::Slider),
    ] {
        set_options_item(tree, scales, id, item, true)?;
    }

    set_node_hidden(tree, gradient, true)?;
    set_node_hidden(tree, bucket, true)?;
    set_node_hidden(tree, smudge, true)?;
    set_node_hidden(tree, paint, true)?;
    Ok(FillControls {
        gradient,
        radial,
        reverse,
        opacity_readout,
        opacity,
        bucket,
        tolerance_readout,
        tolerance,
        contiguous,
        smudge,
        strength_readout,
        strength,
        paint,
        paint_opacity_readout,
        paint_opacity,
        paint_flow_readout,
        paint_flow,
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
        // The Magic Wand shares the Paint Bucket's Tolerance and
        // Contiguous controls (0.214.0).
        (
            fill.bucket,
            matches!(tool, Tool::PaintBucket | Tool::MagicWand),
        ),
        // The Smudge's Strength (0.222.0), shared by the Blur and the
        // Sharpen (0.223.0) and, as Exposure or Flow, by the Dodge, Burn and
        // Sponge (0.224.0), each with its own value; they keep the radius
        // controls.
        (
            fill.smudge,
            matches!(
                tool,
                Tool::Smudge | Tool::Blur | Tool::Sharpen | Tool::Dodge | Tool::Burn | Tool::Sponge
            ),
        ),
        // The Brush's, Pencil's and Clone Stamp's Opacity and Flow (0.228.0);
        // they keep the radius controls.
        (fill.paint, has_paint_opacity(tool)),
    ] {
        changed |= set_item_shown(tree, group, shown)?;
        changed |= set_node_hidden(tree, group, !shown)?;
    }
    // The radius readout has nothing to say for a fill tool, nor (0.215.0)
    // for a selection tool, whose options are the mode group.
    let no_radius = is_fill_tool(tool) || tool.is_selection_tool();
    changed |= set_item_shown(tree, controls.readout, !no_radius)?;
    changed |= set_node_hidden(tree, controls.readout, no_radius)?;

    changed |= sync_checkbox(tree, fill.radial, view.radial)?;
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
    let strength = match tool {
        Tool::Blur => view.blur_strength,
        Tool::Sharpen => view.sharpen_strength,
        Tool::Dodge => view.dodge_exposure,
        Tool::Burn => view.burn_exposure,
        Tool::Sponge => view.sponge_flow,
        _ => view.strength,
    };
    changed |= widgets::set_label_text(
        tree,
        fill.strength_readout,
        &strength_readout_for(tool, strength),
    )?;
    changed |= widgets::set_slider_label(tree, fill.strength, strength_name(tool))?;
    changed |= sync_slider(tree, fill.strength, f64::from(strength), captured)?;

    // The paint group (0.228.0): each tool shows its own values; the Pencil
    // has no Flow, so its Flow readout and slider are hidden (layout and
    // assistive technology).
    let (opacity, flow) = match tool {
        Tool::Pencil => (view.pencil_opacity, view.brush_flow),
        Tool::CloneStamp => (view.clone_opacity, view.clone_flow),
        _ => (view.brush_opacity, view.brush_flow),
    };
    changed |= widgets::set_label_text(
        tree,
        fill.paint_opacity_readout,
        &opacity_readout_text(opacity),
    )?;
    changed |= sync_slider(tree, fill.paint_opacity, f64::from(opacity), captured)?;
    changed |= widgets::set_label_text(tree, fill.paint_flow_readout, &flow_readout_text(flow))?;
    changed |= sync_slider(tree, fill.paint_flow, f64::from(flow), captured)?;
    let flow_shown = has_paint_flow(tool);
    for id in [fill.paint_flow_readout, fill.paint_flow] {
        changed |= set_item_shown(tree, id, flow_shown)?;
        changed |= set_node_hidden(tree, id, !flow_shown)?;
    }
    Ok(changed)
}

/// Whether the active tool's group is shown: `Some(tool)` for the group
/// currently displayed (`Some(Tool::Smudge)` for the Strength group, which
/// the Blur and the Sharpen share, 0.223.0; `Some(Tool::Brush)` for the
/// Opacity and Flow group the Pencil and Clone Stamp share, 0.228.0).
#[must_use]
pub fn shown_fill_group(tree: &WidgetTree<WidgetKind>, controls: &ToolControls) -> Option<Tool> {
    let shown = |id| tree.style(id).is_some_and(|s| s.display != Display::None);
    if shown(controls.fill.gradient) {
        Some(Tool::Gradient)
    } else if shown(controls.fill.bucket) {
        Some(Tool::PaintBucket)
    } else if shown(controls.fill.smudge) {
        Some(Tool::Smudge)
    } else if shown(controls.fill.paint) {
        Some(Tool::Brush)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FillView, STRENGTH_MAX, STRENGTH_MIN, TOLERANCE_MAX, is_fill_tool, opacity_readout_text,
        shown_fill_group, strength_readout_text, sync_fill_controls, tolerance_readout_text,
    };
    use crate::tool::Tool;
    use crate::tool_controls::{ToolControls, insert_tool_controls, tool_controls_contains};
    use crate::workspace::build_workspace;
    use aurora_theme::Scales;
    use aurora_widgets::widgets::WidgetKind;
    use aurora_widgets::{WidgetId, WidgetTree};
    use taffy::Display;

    fn scales() -> Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    const VIEW: FillView = FillView {
        radial: false,
        reverse: false,
        opacity: 100,
        tolerance: 32,
        contiguous: true,
        strength: 50,
        blur_strength: 50,
        sharpen_strength: 50,
        dodge_exposure: 50,
        burn_exposure: 50,
        sponge_flow: 50,
        brush_opacity: 100,
        brush_flow: 100,
        pencil_opacity: 100,
        clone_opacity: 100,
        clone_flow: 100,
    };

    fn built() -> (crate::Workspace, ToolControls) {
        let scales = scales();
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
        controls: &ToolControls,
        tool: Tool,
        view: FillView,
        captured: Option<WidgetId>,
    ) -> bool {
        match sync_fill_controls(&mut ws.tree, controls, tool, view, captured) {
            Ok(changed) => changed,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn shown(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
        tree.style(id).is_some_and(|s| s.display != Display::None)
    }

    fn hidden_to_at(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
        tree.accessibility(id)
            .is_some_and(accesskit::Node::is_hidden)
    }

    #[test]
    fn each_fill_tool_shows_only_its_own_group_and_other_tools_show_neither() {
        let (mut ws, controls) = built();
        // Fresh: nothing shown.
        assert_eq!(shown_fill_group(&ws.tree, &controls), None);
        for (tool, gradient, bucket) in [
            (Tool::Gradient, true, false),
            (Tool::PaintBucket, false, true),
            (Tool::MagicWand, false, true),
            (Tool::Brush, false, false),
            (Tool::Eraser, false, false),
            (Tool::Move, false, false),
        ] {
            sync(&mut ws, &controls, tool, VIEW, None);
            ws.tree.compute_layout(1600.0, 900.0);
            let fill = controls.fill;
            assert_eq!(shown(&ws.tree, fill.gradient), gradient, "{tool:?}");
            assert_eq!(shown(&ws.tree, fill.bucket), bucket, "{tool:?}");
            assert_eq!(hidden_to_at(&ws.tree, fill.gradient), !gradient, "{tool:?}");
            assert_eq!(hidden_to_at(&ws.tree, fill.bucket), !bucket, "{tool:?}");
            // The radius readout belongs to every tool but the fill tools.
            assert_eq!(
                shown(&ws.tree, controls.readout),
                !(is_fill_tool(tool) || tool.is_selection_tool()),
                "{tool:?}"
            );
        }
        sync(&mut ws, &controls, Tool::Gradient, VIEW, None);
        assert_eq!(shown_fill_group(&ws.tree, &controls), Some(Tool::Gradient));
    }

    #[test]
    fn a_shown_group_has_real_size_and_every_widget_is_inside_the_tool_controls() {
        let (mut ws, controls) = built();
        let fill = controls.fill;
        for (tool, ids) in [
            (
                Tool::Gradient,
                vec![fill.radial, fill.reverse, fill.opacity],
            ),
            (Tool::PaintBucket, vec![fill.tolerance, fill.contiguous]),
            (Tool::MagicWand, vec![fill.tolerance, fill.contiguous]),
        ] {
            sync(&mut ws, &controls, tool, VIEW, None);
            ws.tree.compute_layout(1600.0, 900.0);
            for id in ids {
                assert!(tool_controls_contains(&ws.tree, &controls, id));
                let Some(b) = ws.tree.bounds(id) else {
                    unreachable!("laid out");
                };
                assert!(b.width > 0 && b.height > 0, "{tool:?} {id:?} {b:?}");
                let bar = ws.tree.bounds(ws.options_bar).map_or(0, |b| b.right());
                assert!(b.right() <= bar, "{tool:?} {id:?} fits the options bar");
            }
        }
    }

    #[test]
    fn the_widgets_mirror_the_settings_and_a_second_identical_sync_changes_nothing() {
        let (mut ws, controls) = built();
        let view = FillView {
            radial: true,
            reverse: true,
            opacity: 40,
            tolerance: 200,
            contiguous: false,
            strength: 73,
            blur_strength: 50,
            sharpen_strength: 50,
            dodge_exposure: 50,
            burn_exposure: 50,
            sponge_flow: 50,
            brush_opacity: 100,
            brush_flow: 100,
            pencil_opacity: 100,
            clone_opacity: 100,
            clone_flow: 100,
        };
        assert!(sync(&mut ws, &controls, Tool::Gradient, view, None));
        assert!(!sync(&mut ws, &controls, Tool::Gradient, view, None));
        let fill = controls.fill;
        let on = |id| matches!(ws.tree.payload(id), Some(WidgetKind::Checkbox(s)) if s.checked == accesskit::Toggled::True);
        assert!(on(fill.radial), "Radial is checked");
        let slider = |id| match ws.tree.payload(id) {
            Some(WidgetKind::Slider(s)) => s.value,
            other => unreachable!("{other:?}"),
        };
        assert!((slider(fill.opacity) - 40.0).abs() < f64::EPSILON);
        assert!((slider(fill.tolerance) - 200.0).abs() < f64::EPSILON);
        let label = |id| match ws.tree.payload(id) {
            Some(WidgetKind::Label(s)) => s.text.clone(),
            other => unreachable!("{other:?}"),
        };
        assert_eq!(label(fill.opacity_readout), "Opacity 40 %");
        assert_eq!(label(fill.tolerance_readout), "Tolerance 200");
        let checked = |id| match ws.tree.payload(id) {
            Some(WidgetKind::Checkbox(s)) => s.checked,
            other => unreachable!("{other:?}"),
        };
        assert_eq!(checked(fill.reverse), accesskit::Toggled::True);
        assert_eq!(checked(fill.contiguous), accesskit::Toggled::False);
        assert_eq!(opacity_readout_text(100), "Opacity 100 %");
        assert_eq!(tolerance_readout_text(0), "Tolerance 0");
        assert!((TOLERANCE_MAX - 255.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_slider_mid_drag_keeps_its_value_but_the_readout_follows() {
        let (mut ws, controls) = built();
        let fill = controls.fill;
        sync(&mut ws, &controls, Tool::PaintBucket, VIEW, None);
        let moved = FillView {
            tolerance: 90,
            ..VIEW
        };
        sync(
            &mut ws,
            &controls,
            Tool::PaintBucket,
            moved,
            Some(fill.tolerance),
        );
        let value = match ws.tree.payload(fill.tolerance) {
            Some(WidgetKind::Slider(s)) => s.value,
            other => unreachable!("{other:?}"),
        };
        assert!((value - 32.0).abs() < f64::EPSILON, "{value}");
        let text = match ws.tree.payload(fill.tolerance_readout) {
            Some(WidgetKind::Label(s)) => s.text.clone(),
            other => unreachable!("{other:?}"),
        };
        assert_eq!(text, "Tolerance 90");
    }

    #[test]
    fn the_smudge_shows_its_strength_beside_the_radius_and_mirrors_it() {
        let (mut ws, controls) = built();
        let fill = controls.fill;
        let view = FillView {
            strength: 80,
            ..VIEW
        };
        assert!(sync(&mut ws, &controls, Tool::Smudge, view, None));
        ws.tree.compute_layout(1600.0, 900.0);
        assert_eq!(shown_fill_group(&ws.tree, &controls), Some(Tool::Smudge));
        assert!(shown(&ws.tree, fill.smudge));
        assert!(!shown(&ws.tree, fill.gradient));
        assert!(!shown(&ws.tree, fill.bucket));
        assert!(!hidden_to_at(&ws.tree, fill.smudge));
        // The Smudge has a radius: its readout stays.
        assert!(shown(&ws.tree, controls.readout));
        for id in [fill.strength_readout, fill.strength] {
            assert!(tool_controls_contains(&ws.tree, &controls, id));
            let Some(b) = ws.tree.bounds(id) else {
                unreachable!("laid out");
            };
            assert!(b.width > 0 && b.height > 0, "{id:?} {b:?}");
        }
        let value = match ws.tree.payload(fill.strength) {
            Some(WidgetKind::Slider(s)) => s.value,
            other => unreachable!("{other:?}"),
        };
        assert!((value - 80.0).abs() < f64::EPSILON);
        let text = match ws.tree.payload(fill.strength_readout) {
            Some(WidgetKind::Label(s)) => s.text.clone(),
            other => unreachable!("{other:?}"),
        };
        assert_eq!(text, "Strength 80 %");
        assert_eq!(strength_readout_text(1), "Strength 1 %");
        assert!((STRENGTH_MIN - 1.0).abs() < f64::EPSILON);
        assert!((STRENGTH_MAX - 100.0).abs() < f64::EPSILON);
        // Another tool hides it again (layout and assistive technology).
        sync(&mut ws, &controls, Tool::Eraser, view, None);
        assert!(!shown(&ws.tree, fill.smudge));
        assert!(hidden_to_at(&ws.tree, fill.smudge));
        assert_eq!(shown_fill_group(&ws.tree, &controls), None);
    }

    #[test]
    fn the_blur_and_sharpen_share_the_strength_group_each_with_its_own_value() {
        let (mut ws, controls) = built();
        let fill = controls.fill;
        let view = FillView {
            strength: 80,
            blur_strength: 30,
            sharpen_strength: 65,
            ..VIEW
        };
        let read = |ws: &crate::Workspace| {
            let value = match ws.tree.payload(fill.strength) {
                Some(WidgetKind::Slider(s)) => s.value,
                other => unreachable!("{other:?}"),
            };
            let text = match ws.tree.payload(fill.strength_readout) {
                Some(WidgetKind::Label(s)) => s.text.clone(),
                other => unreachable!("{other:?}"),
            };
            (value, text)
        };
        for (tool, value, text) in [
            (Tool::Smudge, 80.0, "Strength 80 %"),
            (Tool::Blur, 30.0, "Strength 30 %"),
            (Tool::Sharpen, 65.0, "Strength 65 %"),
        ] {
            sync(&mut ws, &controls, tool, view, None);
            ws.tree.compute_layout(1600.0, 900.0);
            assert!(shown(&ws.tree, fill.smudge), "{tool:?}");
            assert!(
                shown(&ws.tree, controls.readout),
                "{tool:?} keeps the radius"
            );
            assert!(!hidden_to_at(&ws.tree, fill.smudge), "{tool:?}");
            let (v, t) = read(&ws);
            assert!((v - value).abs() < f64::EPSILON, "{tool:?} {v}");
            assert_eq!(t, text, "{tool:?}");
        }
        sync(&mut ws, &controls, Tool::Eraser, view, None);
        assert!(!shown(&ws.tree, fill.smudge));
    }

    #[test]
    fn the_toning_tools_share_the_group_under_their_own_name_and_value() {
        use super::{strength_name, strength_readout_for};
        let (mut ws, controls) = built();
        let fill = controls.fill;
        let view = FillView {
            dodge_exposure: 21,
            burn_exposure: 64,
            sponge_flow: 77,
            ..VIEW
        };
        assert_eq!(strength_name(Tool::Dodge), "Exposure");
        assert_eq!(strength_name(Tool::Burn), "Exposure");
        assert_eq!(strength_name(Tool::Sponge), "Flow");
        assert_eq!(strength_name(Tool::Smudge), "Strength");
        assert_eq!(strength_name(Tool::Blur), "Strength");
        assert_eq!(strength_readout_for(Tool::Sponge, 5), "Flow 5 %");
        for (tool, name, value) in [
            (Tool::Dodge, "Exposure", 21.0),
            (Tool::Burn, "Exposure", 64.0),
            (Tool::Sponge, "Flow", 77.0),
            (Tool::Smudge, "Strength", 50.0),
        ] {
            sync(&mut ws, &controls, tool, view, None);
            ws.tree.compute_layout(1600.0, 900.0);
            assert!(shown(&ws.tree, fill.smudge), "{tool:?}");
            assert!(
                shown(&ws.tree, controls.readout),
                "{tool:?} keeps the radius"
            );
            let (slider_value, label) = match ws.tree.payload(fill.strength) {
                Some(WidgetKind::Slider(s)) => (s.value, s.label.clone()),
                other => unreachable!("{other:?}"),
            };
            assert!((slider_value - value).abs() < f64::EPSILON, "{tool:?}");
            assert_eq!(label, name, "{tool:?}");
            let node = ws
                .tree
                .accessibility(fill.strength)
                .and_then(|n| n.label().map(str::to_owned));
            assert_eq!(node.as_deref(), Some(name), "{tool:?} accessible name");
            match ws.tree.payload(fill.strength_readout) {
                Some(WidgetKind::Label(s)) => {
                    assert_eq!(s.text, format!("{name} {value} %"), "{tool:?}");
                }
                other => unreachable!("{other:?}"),
            }
        }
    }

    fn paint_view() -> FillView {
        FillView {
            brush_opacity: 40,
            brush_flow: 70,
            pencil_opacity: 55,
            clone_opacity: 25,
            clone_flow: 90,
            ..VIEW
        }
    }

    fn slider_value(ws: &crate::Workspace, id: WidgetId) -> f64 {
        match ws.tree.payload(id) {
            Some(WidgetKind::Slider(s)) => s.value,
            other => unreachable!("{other:?}"),
        }
    }

    fn label_text(ws: &crate::Workspace, id: WidgetId) -> String {
        match ws.tree.payload(id) {
            Some(WidgetKind::Label(s)) => s.text.clone(),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn the_brush_pencil_and_clone_stamp_show_opacity_and_the_others_do_not() {
        let (mut ws, controls) = built();
        let fill = controls.fill;
        for tool in Tool::ALL {
            sync(&mut ws, &controls, tool, paint_view(), None);
            ws.tree.compute_layout(1600.0, 900.0);
            let wants = matches!(tool, Tool::Brush | Tool::Pencil | Tool::CloneStamp);
            assert_eq!(shown(&ws.tree, fill.paint), wants, "{tool:?}");
            assert_eq!(hidden_to_at(&ws.tree, fill.paint), !wants, "{tool:?}");
            assert_eq!(super::has_paint_opacity(tool), wants, "{tool:?}");
            let flow = matches!(tool, Tool::Brush | Tool::CloneStamp);
            assert_eq!(super::has_paint_flow(tool), flow, "{tool:?}");
            if wants {
                for id in [fill.paint_flow_readout, fill.paint_flow] {
                    assert_eq!(shown(&ws.tree, id), flow, "{tool:?} flow {id:?}");
                    assert_eq!(hidden_to_at(&ws.tree, id), !flow, "{tool:?} flow {id:?}");
                }
                for id in [fill.paint_opacity_readout, fill.paint_opacity] {
                    assert!(
                        shown(&ws.tree, id) && !hidden_to_at(&ws.tree, id),
                        "{tool:?}"
                    );
                    assert!(tool_controls_contains(&ws.tree, &controls, id));
                    let Some(b) = ws.tree.bounds(id) else {
                        unreachable!("laid out");
                    };
                    assert!(b.width > 0 && b.height > 0, "{tool:?} {id:?} {b:?}");
                }
                // They keep the radius.
                assert!(shown(&ws.tree, controls.readout), "{tool:?}");
            }
        }
        sync(&mut ws, &controls, Tool::Brush, paint_view(), None);
        assert_eq!(shown_fill_group(&ws.tree, &controls), Some(Tool::Brush));
        sync(&mut ws, &controls, Tool::Move, paint_view(), None);
        assert_eq!(shown_fill_group(&ws.tree, &controls), None);
    }

    #[test]
    fn each_paint_tool_mirrors_its_own_opacity_and_flow() {
        let (mut ws, controls) = built();
        let fill = controls.fill;
        for (tool, opacity, flow) in [
            (Tool::Brush, 40.0, 70.0),
            (Tool::Pencil, 55.0, 70.0),
            (Tool::CloneStamp, 25.0, 90.0),
        ] {
            sync(&mut ws, &controls, tool, paint_view(), None);
            assert!(
                (slider_value(&ws, fill.paint_opacity) - opacity).abs() < f64::EPSILON,
                "{tool:?}"
            );
            assert_eq!(
                label_text(&ws, fill.paint_opacity_readout),
                format!("Opacity {opacity} %")
            );
            if super::has_paint_flow(tool) {
                assert!(
                    (slider_value(&ws, fill.paint_flow) - flow).abs() < f64::EPSILON,
                    "{tool:?}"
                );
                assert_eq!(
                    label_text(&ws, fill.paint_flow_readout),
                    format!("Flow {flow} %")
                );
            }
        }
        assert_eq!(super::flow_readout_text(100), "Flow 100 %");
        // A second identical sync changes nothing.
        assert!(!sync(
            &mut ws,
            &controls,
            Tool::CloneStamp,
            paint_view(),
            None
        ));
    }

    #[test]
    fn the_paint_sliders_are_named_and_ranged_for_assistive_technology() {
        let (ws, controls) = built();
        let fill = controls.fill;
        for (id, name) in [(fill.paint_opacity, "Opacity"), (fill.paint_flow, "Flow")] {
            let node = ws
                .tree
                .accessibility(id)
                .and_then(|n| n.label().map(str::to_owned));
            assert_eq!(node.as_deref(), Some(name));
            match ws.tree.payload(id) {
                Some(WidgetKind::Slider(s)) => {
                    assert!((s.min - STRENGTH_MIN).abs() < f64::EPSILON);
                    assert!((s.max - STRENGTH_MAX).abs() < f64::EPSILON);
                    assert!((s.value - 100.0).abs() < f64::EPSILON, "default 100 %");
                }
                other => unreachable!("{other:?}"),
            }
        }
    }

    #[test]
    fn a_paint_slider_mid_drag_keeps_its_value_but_the_readout_follows() {
        let (mut ws, controls) = built();
        let fill = controls.fill;
        sync(&mut ws, &controls, Tool::Brush, VIEW, None);
        let moved = FillView {
            brush_opacity: 30,
            ..VIEW
        };
        sync(
            &mut ws,
            &controls,
            Tool::Brush,
            moved,
            Some(fill.paint_opacity),
        );
        assert!((slider_value(&ws, fill.paint_opacity) - 100.0).abs() < f64::EPSILON);
        assert_eq!(label_text(&ws, fill.paint_opacity_readout), "Opacity 30 %");
    }
}
