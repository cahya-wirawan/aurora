//! The Properties panel's **editable** tool controls (0.136.0): a live
//! "Radius 24 px" readout and a radius slider for the active tool — the
//! first Properties-panel widgets that edit anything. PLAN.md M1.8,
//! "Layers, history, tool-options panels".
//!
//! **Where they live.** Since 0.160.0 the radius readout and slider are
//! in the workspace's **options bar** ([`crate::Workspace::options_bar`],
//! Photoshop's layout convention), in their own `options` strip — moved,
//! not mirrored, so there is still exactly one slider and one
//! `ToolSettings` behind it. A tool with no radius hides the slider and
//! keeps only the readout. The Curves editor stays in the Properties
//! panel: the strip there ([`ToolControls::root`]) is a second child of
//! the panel's own *root*, after the body, for the same reason as
//! [`crate::layer_controls`] — [`crate::populate_properties_panel`]
//! clears the whole of `panel.body` on every tool switch — and
//! [`crate::set_panel_collapsed`] hides it with the body (the options
//! bar is unaffected by collapsing the panel).
//!
//! **This module knows no tool parameters.** Which tools have a radius,
//! and what it is, is `aurora-app`'s knowledge (its `ToolSettings`); the
//! caller passes `Some(radius)` for a tool that has one and `None` for
//! one that does not, and [`sync_tool_controls`] mirrors that in place.
//! Tool settings are not document state: nothing here (or in the app)
//! records an undo step for them.
//!
//! **The range is an engineering default, not a design decision.**
//! [`TOOL_RADIUS_MIN`]..=[`TOOL_RADIUS_MAX`] (1 to 256 px) was chosen so
//! a dab stays a handful of tiles wide; the range, the readout's wording
//! and whether it should show a diameter ("Size") rather than a radius
//! are flagged to the design owner (PRD FR-027 *Ownership*).

use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{Display, FlexDirection, Rect as LayoutRect, Size, Style};

use crate::curves_controls::{CurvesControls, insert_curves_controls};
use crate::panel::{PanelHandle, panel_is_collapsed};
use crate::tool::Tool;

/// The smallest radius the slider offers, in document pixels.
pub const TOOL_RADIUS_MIN: f64 = 1.0;
/// The largest radius the slider offers, in document pixels.
pub const TOOL_RADIUS_MAX: f64 = 256.0;

/// The readout and the slider, plus the strip that holds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolControls {
    /// The Properties-panel strip — a child of the Properties panel's
    /// root — holding [`Self::curves`].
    pub root: WidgetId,
    /// The options-bar strip (0.160.0) holding [`Self::readout`] and
    /// [`Self::radius`] — a child of [`crate::Workspace::options_bar`].
    pub options: WidgetId,
    /// The live readout ("Radius 24 px").
    pub readout: WidgetId,
    /// The radius slider, in pixels
    /// ([`TOOL_RADIUS_MIN`]..=[`TOOL_RADIUS_MAX`]).
    pub radius: WidgetId,
    /// The Curves editor (0.156.0) — the strip's last child, hidden
    /// unless the active layer is a Curves adjustment
    /// ([`crate::sync_curves_controls`]). Inside the strip so
    /// [`tool_controls_contains`] routes its events here too.
    pub curves: CurvesControls,
}

/// The readout's text for `tool` with `radius` (`None`: the tool has no
/// radius). A radius is shown to whole pixels.
#[must_use]
pub fn radius_readout(tool: Tool, radius: Option<f64>) -> String {
    match radius {
        Some(radius) if radius.is_finite() => format!("Radius {radius:.0} px"),
        _ => format!("No radius for {}", tool.label()),
    }
}

fn strip_style(scales: &Scales, collapsed: bool) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let pad = length(scales.spacing.sm as f32);
    #[allow(clippy::cast_precision_loss)]
    let gap = length(scales.spacing.xs as f32);
    Style {
        display: if collapsed {
            Display::None
        } else {
            Display::Flex
        },
        flex_direction: FlexDirection::Column,
        flex_shrink: 0.0,
        gap: Size {
            width: gap,
            height: gap,
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

/// The options-bar strip's style: a row filling the bar, its readout and
/// slider `spacing.sm` apart and vertically centred.
fn options_style(scales: &Scales) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let gap = length(scales.spacing.sm as f32);
    Style {
        flex_direction: FlexDirection::Row,
        flex_grow: 1.0,
        align_items: Some(taffy::AlignItems::CENTER),
        gap: Size {
            width: gap,
            height: gap,
        },
        min_size: Size {
            width: <taffy::Dimension as taffy::style_helpers::TaffyZero>::ZERO,
            height: <taffy::Dimension as taffy::style_helpers::TaffyZero>::ZERO,
        },
        ..Default::default()
    }
}

/// How an options-row item is sized: the readout takes the row's spare
/// width (from a zero basis — a `Label`'s own width is `auto`, which in a
/// row would be zero text-blind); the slider is exactly
/// `size.options_control_width` wide (0.160.0 review: a design token, the
/// design owner's decision, replacing a 1:2 flex split), shrinking only
/// if the bar is narrower than that.
#[derive(Debug, Clone, Copy)]
enum OptionsItem {
    Readout,
    Slider,
}

/// Re-styles `id` (already built) for the options row as `item`, shown or
/// hidden. Returns whether its style changed.
fn set_options_item(
    tree: &mut WidgetTree<WidgetKind>,
    scales: &Scales,
    id: WidgetId,
    item: OptionsItem,
    shown: bool,
) -> Result<bool, WidgetError> {
    let current = tree
        .style(id)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(id))?;
    let zero = <taffy::Dimension as taffy::style_helpers::TaffyZero>::ZERO;
    let mut style = current.clone();
    style.display = if shown { Display::Flex } else { Display::None };
    style.flex_shrink = 1.0;
    style.min_size.width = zero;
    match item {
        OptionsItem::Readout => {
            style.flex_grow = 1.0;
            style.flex_basis = zero;
        }
        OptionsItem::Slider => {
            #[allow(clippy::cast_precision_loss)]
            let width = length(scales.size.options_control_width as f32);
            style.flex_grow = 0.0;
            style.flex_basis = taffy::Dimension::auto();
            style.size.width = width;
        }
    }
    if style == current {
        return Ok(false);
    }
    tree.set_style(id, style)?;
    Ok(true)
}

/// Shows or hides an options-row item (`Display`), leaving its sizing as
/// [`set_options_item`] set it. Returns whether its style changed.
fn set_item_shown(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    shown: bool,
) -> Result<bool, WidgetError> {
    let mut style = tree
        .style(id)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(id))?;
    let display = if shown { Display::Flex } else { Display::None };
    if style.display == display {
        return Ok(false);
    }
    style.display = display;
    tree.set_style(id, style)?;
    Ok(true)
}

/// Marks `id`'s accessibility node hidden (or not), so a slider hidden by
/// layout is not announced (0.160.0 review J1, the precedent of a linked
/// scrollbar that fits). Applied last, after any mutator that rebuilds
/// the node from widget state. Returns whether anything changed.
fn set_node_hidden(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    hidden: bool,
) -> Result<bool, WidgetError> {
    let node = tree
        .accessibility(id)
        .ok_or(WidgetError::UnknownWidget(id))?;
    if node.is_hidden() == hidden {
        return Ok(false);
    }
    let mut updated = node.clone();
    if hidden {
        updated.set_hidden();
    } else {
        updated.clear_hidden();
    }
    tree.set_accessibility(id, updated)?;
    Ok(true)
}

/// Builds the controls: the radius readout and slider into
/// `options_bar` (the workspace's options bar), and the Curves editor
/// into `panel` (the Properties panel) as the last child of its root —
/// outside `panel.body`, see this module's doc comment. They start
/// disabled, the slider hidden, showing the smallest radius; call
/// [`sync_tool_controls`] to show a real tool. Inserted into a panel
/// that is already collapsed, the Properties strip starts hidden.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `options_bar`, `panel.root`
/// or `panel.body` doesn't exist. On any failure the partly built strips
/// are removed again.
pub fn insert_tool_controls(
    tree: &mut WidgetTree<WidgetKind>,
    options_bar: WidgetId,
    panel: PanelHandle,
    scales: &Scales,
) -> Result<ToolControls, WidgetError> {
    let collapsed = panel_is_collapsed(tree, panel)?;
    let options = widgets::insert_container(tree, options_bar, options_style(scales))?;
    let root = match widgets::insert_container(tree, panel.root, strip_style(scales, collapsed)) {
        Ok(root) => root,
        Err(err) => {
            let _ = tree.remove(options);
            return Err(err);
        }
    };
    let built = build(tree, options, root, scales);
    if built.is_err() {
        let _ = tree.remove(root);
        let _ = tree.remove(options);
    }
    built
}

fn build(
    tree: &mut WidgetTree<WidgetKind>,
    options: WidgetId,
    root: WidgetId,
    scales: &Scales,
) -> Result<ToolControls, WidgetError> {
    let readout = widgets::insert_label(tree, options, scales, radius_readout(Tool::Move, None))?;
    let radius = widgets::insert_slider(
        tree,
        options,
        scales,
        "Radius",
        TOOL_RADIUS_MIN,
        TOOL_RADIUS_MIN,
        TOOL_RADIUS_MAX,
    )?;
    set_options_item(tree, scales, readout, OptionsItem::Readout, true)?;
    set_options_item(tree, scales, radius, OptionsItem::Slider, false)?;
    let curves = insert_curves_controls(tree, root, scales)?;
    let controls = ToolControls {
        root,
        options,
        readout,
        radius,
        curves,
    };
    set_disabled(tree, controls, true)?;
    set_node_hidden(tree, radius, true)?;
    Ok(controls)
}

/// Whether the options bar is showing the radius slider (0.160.0): only
/// for a tool that has a radius.
#[must_use]
pub fn radius_slider_shown(tree: &WidgetTree<WidgetKind>, controls: &ToolControls) -> bool {
    tree.style(controls.radius)
        .is_some_and(|style| style.display != Display::None)
}

fn set_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    controls: ToolControls,
    disabled: bool,
) -> Result<bool, WidgetError> {
    let mut changed = widgets::set_label_disabled(tree, controls.readout, disabled)?;
    match tree.payload(controls.radius) {
        Some(WidgetKind::Slider(state)) if state.disabled == disabled => {}
        Some(WidgetKind::Slider(_)) => {
            widgets::set_slider_disabled(tree, controls.radius, disabled)?;
            changed = true;
        }
        Some(_) => return Err(WidgetError::WrongWidgetKind(controls.radius)),
        None => return Err(WidgetError::UnknownWidget(controls.radius)),
    }
    Ok(changed)
}

/// Mirrors the active `tool` and its `radius` into `controls`, in place
/// — never inserting or removing a widget, so the ids a caller holds (and
/// a pointer capture on the slider) stay valid. Returns whether anything
/// changed; a control already showing the right state is not touched,
/// so this is cheap enough to run on every event-loop iteration.
///
/// `None` (the tool has no radius — `Move`, `Zoom`, ...) disables both
/// controls and says so in the readout; the slider keeps its last value.
/// `Some` re-enables them *first* (`set_slider_value` refuses a disabled
/// slider) and shows the radius, clamped to the slider's range; a
/// non-finite radius is treated as `None`.
///
/// `captured` is the widget holding pointer capture: while it is the
/// slider, the slider's value is **not** overwritten (mid-drag it is the
/// source of truth), but the readout always follows `radius`.
///
/// # Errors
///
/// Returns whatever the underlying widget setters return for an id that
/// isn't the widget kind `controls` says it is.
pub fn sync_tool_controls(
    tree: &mut WidgetTree<WidgetKind>,
    controls: ToolControls,
    tool: Tool,
    radius: Option<f64>,
    captured: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    let radius = radius.filter(|radius| radius.is_finite());
    let mut changed =
        widgets::set_label_text(tree, controls.readout, &radius_readout(tool, radius))?;
    let Some(radius) = radius else {
        changed |= set_item_shown(tree, controls.radius, false)?;
        changed |= set_disabled(tree, controls, true)?;
        changed |= set_node_hidden(tree, controls.radius, true)?;
        return Ok(changed);
    };
    changed |= set_item_shown(tree, controls.radius, true)?;
    changed |= set_disabled(tree, controls, false)?;
    changed |= set_node_hidden(tree, controls.radius, false)?;
    if captured != Some(controls.radius) {
        let target = radius.clamp(TOOL_RADIUS_MIN, TOOL_RADIUS_MAX);
        let shown = match tree.payload(controls.radius) {
            Some(WidgetKind::Slider(state)) => state.value,
            _ => return Err(WidgetError::WrongWidgetKind(controls.radius)),
        };
        if shown.to_bits() != target.to_bits() {
            widgets::set_slider_value(tree, controls.radius, target)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// Whether `id` is (or lies inside) either of `controls`' strips — the
/// options-bar one or the Properties-panel one.
#[must_use]
pub fn tool_controls_contains(
    tree: &WidgetTree<WidgetKind>,
    controls: &ToolControls,
    id: WidgetId,
) -> bool {
    tree.is_within(controls.root, id) || tree.is_within(controls.options, id)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{
        TOOL_RADIUS_MAX, TOOL_RADIUS_MIN, ToolControls, insert_tool_controls, radius_readout,
        sync_tool_controls,
    };
    use crate::properties_panel::populate_properties_panel;
    use crate::tool::Tool;
    use crate::workspace::{build_workspace, set_rail_width};
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

    fn built(width: f32, height: f32) -> (crate::Workspace, ToolControls) {
        let scales = scales();
        let mut workspace = build_workspace(&scales);
        let controls = match insert_tool_controls(
            &mut workspace.tree,
            workspace.options_bar,
            workspace.properties,
            &scales,
        ) {
            Ok(controls) => controls,
            Err(err) => unreachable!("{err:?}"),
        };
        workspace.tree.compute_layout(width, height);
        (workspace, controls)
    }

    fn slider(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> (f64, bool) {
        match tree.payload(id) {
            Some(WidgetKind::Slider(state)) => (state.value, state.disabled),
            other => unreachable!("expected Slider, got {other:?}"),
        }
    }

    fn readout(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> (String, bool) {
        match tree.payload(id) {
            Some(WidgetKind::Label(state)) => (state.text.clone(), state.disabled),
            other => unreachable!("expected Label, got {other:?}"),
        }
    }

    fn sync(
        tree: &mut WidgetTree<WidgetKind>,
        controls: ToolControls,
        tool: Tool,
        radius: Option<f64>,
        captured: Option<WidgetId>,
    ) -> bool {
        match sync_tool_controls(tree, controls, tool, radius, captured) {
            Ok(changed) => changed,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn the_strip_survives_repopulating_the_properties_panel() {
        let scales = scales();
        let (mut ws, controls) = built(1600.0, 900.0);
        let options = [("Radius", "24px".to_owned())];
        for tool in [Tool::Brush, Tool::Move, Tool::Eraser] {
            if let Err(err) =
                populate_properties_panel(&mut ws.tree, ws.properties, &scales, tool, &options)
            {
                unreachable!("{err:?}");
            }
        }
        for id in [controls.root, controls.curves.root] {
            assert!(ws.tree.contains(id), "populating must not remove the strip");
            assert!(!ws.tree.is_within(ws.properties.body, id));
            assert!(ws.tree.is_within(ws.properties.root, id));
        }
        // 0.160.0: the radius controls moved to the options bar.
        for id in [controls.options, controls.readout, controls.radius] {
            assert!(
                ws.tree.contains(id),
                "populating must not remove the options"
            );
            assert!(ws.tree.is_within(ws.options_bar, id));
            assert!(!ws.tree.is_within(ws.properties.root, id));
        }
    }

    /// 0.160.0, AC-3: the options bar shows the active tool's options —
    /// the radius slider for Brush and Eraser only, hidden (and not
    /// hittable) for every other tool, with the readout always present.
    #[test]
    fn the_options_bar_shows_the_radius_slider_only_for_a_tool_with_a_radius() {
        let (mut ws, controls) = built(1600.0, 900.0);
        assert!(
            !super::radius_slider_shown(&ws.tree, &controls),
            "starts hidden"
        );
        for tool in Tool::ALL {
            let radius = matches!(tool, Tool::Brush | Tool::Eraser).then_some(24.0);
            let _ = sync(&mut ws.tree, controls, tool, radius, None);
            ws.tree.compute_layout(1600.0, 900.0);
            assert_eq!(
                super::radius_slider_shown(&ws.tree, &controls),
                radius.is_some(),
                "{tool:?}"
            );
            // Review J1: a hidden slider is hidden from assistive
            // technology too, and announced again once shown.
            assert_eq!(
                ws.tree
                    .accessibility(controls.radius)
                    .map(accesskit::Node::is_hidden),
                Some(radius.is_none()),
                "{tool:?}: the slider's node is hidden exactly when the slider is"
            );
            let Some(bar) = ws.tree.bounds(ws.options_bar) else {
                unreachable!("laid out");
            };
            let Some(text) = ws.tree.bounds(controls.readout) else {
                unreachable!("laid out");
            };
            assert!(text.width > 0, "{tool:?}: the readout is always shown");
            assert!(
                text.y >= bar.y && text.y + i64::from(text.height) <= bar.y + i64::from(bar.height),
                "{tool:?}: the readout sits inside the bar: {text:?} {bar:?}"
            );
            if radius.is_some() {
                assert_hittable(&ws.tree, &controls);
            } else {
                assert_eq!(
                    ws.tree.bounds(controls.radius).map(|b| b.width),
                    Some(0),
                    "{tool:?}: a hidden slider has no extent"
                );
            }
        }
    }

    /// Review revision (design owner, 2026-10-09): the options-bar slider
    /// is exactly `size.options_control_width` wide when the bar has room,
    /// and the readout takes the rest of the row.
    #[test]
    fn the_options_bar_slider_width_follows_the_size_token() {
        let scales = scales();
        let (mut ws, controls) = built(1600.0, 900.0);
        let _ = sync(&mut ws.tree, controls, Tool::Brush, Some(24.0), None);
        ws.tree.compute_layout(1600.0, 900.0);
        let width = |ws: &crate::Workspace, id| ws.tree.bounds(id).map(|b| b.width);
        assert_eq!(
            width(&ws, controls.radius),
            Some(scales.size.options_control_width)
        );
        assert!(
            width(&ws, controls.readout).is_some_and(|w| w > scales.size.options_control_width),
            "the readout takes the rest of a wide bar"
        );
        let mut wider = scales.clone();
        wider.size.options_control_width += 40;
        let mut other = build_workspace(&wider);
        let controls = match insert_tool_controls(
            &mut other.tree,
            other.options_bar,
            other.properties,
            &wider,
        ) {
            Ok(controls) => controls,
            Err(err) => unreachable!("{err:?}"),
        };
        let _ = sync(&mut other.tree, controls, Tool::Brush, Some(24.0), None);
        other.tree.compute_layout(1600.0, 900.0);
        assert_eq!(
            width(&other, controls.radius),
            Some(wider.size.options_control_width),
            "the width is read from the token, not fixed"
        );
    }

    /// One slider, not two (AC-3): nothing in the Properties panel is a
    /// slider once the radius moved out of it.
    #[test]
    fn the_properties_panel_holds_no_radius_slider_any_more() {
        let (ws, _controls) = built(1600.0, 900.0);
        let mut stack = vec![ws.properties.root];
        let mut sliders = Vec::new();
        while let Some(id) = stack.pop() {
            stack.extend(ws.tree.children(id).unwrap_or_default().iter().copied());
            if let Some(WidgetKind::Slider(state)) = ws.tree.payload(id)
                && ws.tree.accessibility(id).and_then(accesskit::Node::label) == Some("Radius")
            {
                sliders.push(state.value);
            }
        }
        assert!(sliders.is_empty(), "{sliders:?}");
    }

    fn assert_hittable(tree: &WidgetTree<WidgetKind>, controls: &ToolControls) {
        for id in [controls.readout, controls.radius] {
            let Some(bounds) = tree.bounds(id) else {
                unreachable!("laid out");
            };
            assert!(
                bounds.width > 0 && bounds.height > 0,
                "{id:?} has {bounds:?}"
            );
            let x = bounds.x + i64::from(bounds.width / 2);
            let y = bounds.y + i64::from(bounds.height / 2);
            #[allow(clippy::cast_precision_loss)]
            let hit = tree.hit_test((x as f32, y as f32));
            assert!(
                hit.is_some_and(|hit| tree.is_within(id, hit)),
                "{id:?} at ({x}, {y}) hit {hit:?}"
            );
        }
    }

    #[test]
    fn the_controls_are_hittable_at_the_default_and_the_narrowest_rail() {
        let scales = scales();
        let (mut ws, controls) = built(1600.0, 900.0);
        let options = [("Radius", "24px".to_owned())];
        if let Err(err) =
            populate_properties_panel(&mut ws.tree, ws.properties, &scales, Tool::Brush, &options)
        {
            unreachable!("{err:?}");
        }
        let _ = sync(&mut ws.tree, controls, Tool::Brush, Some(24.0), None);
        ws.tree.compute_layout(1600.0, 900.0);
        assert_hittable(&ws.tree, &controls);
        if let Err(err) = set_rail_width(&mut ws.tree, ws.rail, ws.divider, 150.0) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);
        assert_hittable(&ws.tree, &controls);
    }

    #[test]
    fn collapsing_the_properties_panel_hides_the_strip() {
        // 0.160.0: the Properties strip (the Curves editor's) hides with
        // the panel; the radius controls, in the options bar, stay.
        let (mut ws, controls) = built(1600.0, 900.0);
        let _ = sync(&mut ws.tree, controls, Tool::Brush, Some(24.0), None);
        if let Err(err) = crate::set_panel_collapsed(&mut ws.tree, ws.properties, true) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);
        assert_eq!(
            ws.tree.style(controls.root).map(|style| style.display),
            Some(Display::None)
        );
        assert_hittable(&ws.tree, &controls);
        if let Err(err) = crate::set_panel_collapsed(&mut ws.tree, ws.properties, false) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);
        assert_ne!(
            ws.tree.style(controls.root).map(|style| style.display),
            Some(Display::None)
        );
        assert_hittable(&ws.tree, &controls);
    }

    #[test]
    fn inserting_into_a_collapsed_panel_starts_hidden() {
        let scales = scales();
        let mut ws = build_workspace(&scales);
        if let Err(err) = crate::set_panel_collapsed(&mut ws.tree, ws.properties, true) {
            unreachable!("{err:?}");
        }
        let controls =
            match insert_tool_controls(&mut ws.tree, ws.options_bar, ws.properties, &scales) {
                Ok(controls) => controls,
                Err(err) => unreachable!("{err:?}"),
            };
        assert_eq!(
            ws.tree.style(controls.root).map(|style| style.display),
            Some(Display::None)
        );
    }

    #[test]
    fn sync_reflects_the_tool_and_disables_for_a_tool_without_a_radius() {
        let (mut ws, controls) = built(1600.0, 900.0);
        assert!(slider(&ws.tree, controls.radius).1, "starts disabled");
        assert!(sync(&mut ws.tree, controls, Tool::Brush, Some(40.0), None));
        assert_eq!(slider(&ws.tree, controls.radius), (40.0, false));
        assert_eq!(
            readout(&ws.tree, controls.readout),
            ("Radius 40 px".to_owned(), false)
        );
        assert_eq!(
            ws.tree
                .accessibility(controls.readout)
                .and_then(|node| node.label()),
            Some("Radius 40 px")
        );
        assert!(
            !sync(&mut ws.tree, controls, Tool::Brush, Some(40.0), None),
            "a second sync with nothing changed touches nothing"
        );
        assert!(sync(&mut ws.tree, controls, Tool::Move, None, None));
        let (value, disabled) = slider(&ws.tree, controls.radius);
        assert!(disabled);
        assert!((value - 40.0).abs() < 1e-9, "keeps its last value");
        assert_eq!(
            readout(&ws.tree, controls.readout),
            ("No radius for Move".to_owned(), true)
        );
        assert!(sync(&mut ws.tree, controls, Tool::Eraser, Some(7.0), None));
        assert_eq!(slider(&ws.tree, controls.radius), (7.0, false));
    }

    #[test]
    fn sync_clamps_to_the_range_and_treats_non_finite_as_no_radius() {
        let (mut ws, controls) = built(1600.0, 900.0);
        sync(&mut ws.tree, controls, Tool::Brush, Some(10_000.0), None);
        assert_eq!(slider(&ws.tree, controls.radius).0, TOOL_RADIUS_MAX);
        sync(&mut ws.tree, controls, Tool::Brush, Some(0.0), None);
        assert_eq!(slider(&ws.tree, controls.radius).0, TOOL_RADIUS_MIN);
        sync(&mut ws.tree, controls, Tool::Brush, Some(f64::NAN), None);
        assert!(slider(&ws.tree, controls.radius).1);
        assert_eq!(
            radius_readout(Tool::Brush, Some(f64::INFINITY)),
            "No radius for Brush"
        );
    }

    #[test]
    fn sync_skips_the_slider_value_while_it_holds_capture_but_updates_the_readout() {
        let (mut ws, controls) = built(1600.0, 900.0);
        sync(&mut ws.tree, controls, Tool::Brush, Some(24.0), None);
        if let Err(err) =
            aurora_widgets::widgets::set_slider_value(&mut ws.tree, controls.radius, 90.5)
        {
            unreachable!("{err:?}");
        }
        sync(
            &mut ws.tree,
            controls,
            Tool::Brush,
            Some(31.0),
            Some(controls.radius),
        );
        assert!((slider(&ws.tree, controls.radius).0 - 90.5).abs() < 1e-9);
        assert_eq!(readout(&ws.tree, controls.readout).0, "Radius 31 px");
        sync(&mut ws.tree, controls, Tool::Brush, Some(31.0), None);
        assert!((slider(&ws.tree, controls.radius).0 - 31.0).abs() < 1e-9);
    }
}
