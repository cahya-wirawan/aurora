//! The Layers panel's **editable** controls (0.135.0): an opacity slider,
//! a blend-mode dropdown and a visibility checkbox for the *active* layer
//! — the first panel widgets in the running app that edit a live
//! document. PLAN.md M1.8, "Layers, history, tool-options panels".
//!
//! **Where they live.** [`crate::populate_layers_panel`] clears the whole
//! of `panel.body` on every call (it is the only way layer rows are
//! built), so anything inside the body would be destroyed and re-created
//! with fresh `WidgetId`s on every structural change — including mid-drag.
//! The strip is therefore a second child of the panel's own *root*, a
//! sibling after the body: it survives every repopulation, and
//! [`crate::set_panel_collapsed`] hides it with the body. It sits
//! **below** the layer list rather than above it, because `WidgetTree`
//! only appends children and the body already exists by the time the
//! strip is inserted. `flex_shrink: 0` keeps it at its content height
//! when the rail is short; the body (`flex_basis: 0`) gives way first.
//!
//! **This module knows nothing about undo.** It builds the controls,
//! mirrors a document's state into them ([`sync_layer_controls`]), and
//! names the blend modes ([`blend_mode_label`]). Turning a widget outcome
//! into a document edit — live opacity drag, one undo step per gesture —
//! is `aurora-app`'s job, the same split the gallery panel draws.

use accesskit::Toggled;
use aurora_doc::{BlendMode, LayerId, LayerTree};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{FlexDirection, Rect as LayoutRect, Size, Style};

use crate::panel::PanelHandle;

/// The three controls, plus the strip that holds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerControls {
    /// The strip container — a child of the Layers panel's root.
    pub root: WidgetId,
    /// Opacity, in percent (`0..=100`).
    pub opacity: WidgetId,
    /// Blend mode; option `i` is `BlendMode::ALL[i]`.
    pub blend: WidgetId,
    /// Whether the active layer is visible.
    pub visible: WidgetId,
}

/// The blend-mode dropdown's options, in `BlendMode::ALL` order — the one
/// place that order is turned into labels, so an option index and
/// `BlendMode::ALL[index]` can never drift apart.
#[must_use]
pub fn blend_mode_options() -> Vec<String> {
    BlendMode::ALL
        .iter()
        .map(|&mode| blend_mode_label(mode).to_owned())
        .collect()
}

/// The dropdown option index of `mode` (its position in `BlendMode::ALL`).
#[must_use]
pub fn blend_mode_index(mode: BlendMode) -> Option<usize> {
    BlendMode::ALL
        .iter()
        .position(|&candidate| candidate == mode)
}

/// The user-facing name of `mode` — Photoshop's own menu names, since a
/// PSD user already knows them. An exhaustive `match`, so a new variant
/// cannot ship without a name.
#[must_use]
pub const fn blend_mode_label(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "Normal",
        BlendMode::Dissolve => "Dissolve",
        BlendMode::Darken => "Darken",
        BlendMode::Multiply => "Multiply",
        BlendMode::ColorBurn => "Color Burn",
        BlendMode::LinearBurn => "Linear Burn",
        BlendMode::DarkerColor => "Darker Color",
        BlendMode::Lighten => "Lighten",
        BlendMode::Screen => "Screen",
        BlendMode::ColorDodge => "Color Dodge",
        BlendMode::LinearDodge => "Linear Dodge (Add)",
        BlendMode::LighterColor => "Lighter Color",
        BlendMode::Overlay => "Overlay",
        BlendMode::SoftLight => "Soft Light",
        BlendMode::HardLight => "Hard Light",
        BlendMode::VividLight => "Vivid Light",
        BlendMode::LinearLight => "Linear Light",
        BlendMode::PinLight => "Pin Light",
        BlendMode::HardMix => "Hard Mix",
        BlendMode::Difference => "Difference",
        BlendMode::Exclusion => "Exclusion",
        BlendMode::Subtract => "Subtract",
        BlendMode::Divide => "Divide",
        BlendMode::Hue => "Hue",
        BlendMode::Saturation => "Saturation",
        BlendMode::Color => "Color",
        BlendMode::Luminosity => "Luminosity",
    }
}

fn strip_style(scales: &Scales) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let pad = length(scales.spacing.sm as f32);
    #[allow(clippy::cast_precision_loss)]
    let gap = length(scales.spacing.sm as f32);
    Style {
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

/// Adds the controls strip to `panel` (the Layers panel) as the last
/// child of its root — outside `panel.body`, see this module's doc
/// comment. The controls start disabled, showing `Normal`, `100%` and
/// visible; call [`sync_layer_controls`] to show a real layer.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.root` doesn't exist.
/// On any failure the partly built strip is removed again.
pub fn insert_layer_controls(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
) -> Result<LayerControls, WidgetError> {
    let root = widgets::insert_container(tree, panel.root, strip_style(scales))?;
    let built = build(tree, root, scales);
    if built.is_err() {
        let _ = tree.remove(root);
    }
    built
}

fn build(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    scales: &Scales,
) -> Result<LayerControls, WidgetError> {
    let blend = widgets::insert_dropdown(
        tree,
        root,
        scales,
        "Blend mode",
        blend_mode_options(),
        blend_mode_index(BlendMode::Normal),
    )?;
    let opacity = widgets::insert_slider(tree, root, scales, "Opacity", 100.0, 0.0, 100.0)?;
    let visible = widgets::insert_checkbox(tree, root, scales, "Visible")?;
    widgets::set_checkbox_checked(tree, visible, Toggled::True)?;
    let controls = LayerControls {
        root,
        opacity,
        blend,
        visible,
    };
    set_disabled(tree, controls, true)?;
    Ok(controls)
}

fn set_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    controls: LayerControls,
    disabled: bool,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    if let Some(WidgetKind::Slider(state)) = tree.payload(controls.opacity)
        && state.disabled != disabled
    {
        widgets::set_slider_disabled(tree, controls.opacity, disabled)?;
        changed = true;
    }
    if widgets::dropdown_state(tree, controls.blend)?.is_disabled() != disabled {
        widgets::set_dropdown_disabled(tree, controls.blend, disabled)?;
        changed = true;
    }
    if let Some(WidgetKind::Checkbox(state)) = tree.payload(controls.visible)
        && state.disabled != disabled
    {
        widgets::set_checkbox_disabled(tree, controls.visible, disabled)?;
        changed = true;
    }
    Ok(changed)
}

/// Mirrors `active`'s opacity, blend mode and visibility into
/// `controls`, in place — never inserting or removing a widget, so the
/// ids a caller holds (and a pointer capture on the slider) stay valid.
/// Returns whether anything changed; a control already showing the right
/// state is not touched at all, so this is cheap enough for a caller to
/// run on every event-loop iteration as a catch-all.
///
/// `None`, or an id `layers` doesn't know, disables all three (their
/// shown values are left as they were). `Some` re-enables them *first*,
/// since `set_slider_value` refuses a disabled slider.
///
/// `captured` is the widget currently holding pointer capture: when it
/// is the opacity slider, the slider's value is **not** overwritten —
/// mid-drag it is the source of truth and the document follows it, and
/// writing the document's value back would fight the pointer.
///
/// # Errors
///
/// Returns whatever the underlying widget setters return for an id that
/// isn't the widget kind `controls` says it is.
pub fn sync_layer_controls(
    tree: &mut WidgetTree<WidgetKind>,
    controls: LayerControls,
    layers: &LayerTree,
    active: Option<LayerId>,
    captured: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    let Some(id) = active.filter(|&id| layers.contains(id)) else {
        return set_disabled(tree, controls, true);
    };
    let mut changed = set_disabled(tree, controls, false)?;
    if captured != Some(controls.opacity) {
        let target = f64::from(layers.opacity(id).unwrap_or(1.0)) * 100.0;
        let shown = match tree.payload(controls.opacity) {
            Some(WidgetKind::Slider(state)) => state.value,
            _ => return Err(WidgetError::WrongWidgetKind(controls.opacity)),
        };
        if shown.to_bits() != target.to_bits() {
            widgets::set_slider_value(tree, controls.opacity, target)?;
            changed = true;
        }
    }
    let index = blend_mode_index(layers.blend_mode(id).unwrap_or_default());
    if widgets::dropdown_state(tree, controls.blend)?.selected() != index {
        widgets::set_dropdown_selected(tree, controls.blend, index)?;
        changed = true;
    }
    let checked = if layers.visible(id).unwrap_or(true) {
        Toggled::True
    } else {
        Toggled::False
    };
    if let Some(WidgetKind::Checkbox(state)) = tree.payload(controls.visible)
        && state.checked != checked
    {
        widgets::set_checkbox_checked(tree, controls.visible, checked)?;
        changed = true;
    }
    Ok(changed)
}

/// Which of `controls`' widgets `id` is, or lies inside (an open
/// dropdown's list and rows are descendants of the dropdown).
#[must_use]
pub fn layer_controls_contains(
    tree: &WidgetTree<WidgetKind>,
    controls: &LayerControls,
    id: WidgetId,
) -> bool {
    tree.is_within(controls.root, id)
}

#[cfg(test)]
mod tests {
    use super::{
        LayerControls, blend_mode_index, blend_mode_label, blend_mode_options,
        insert_layer_controls, sync_layer_controls,
    };
    use crate::layers_panel::populate_layers_panel;
    use crate::workspace::{build_workspace, set_rail_width};
    use accesskit::Toggled;
    use aurora_core::Rect;
    use aurora_doc::{BlendMode, LayerId, LayerTree};
    use aurora_theme::Scales;
    use aurora_widgets::widgets::WidgetKind;
    use aurora_widgets::{WidgetId, WidgetTree};
    use std::collections::HashSet;
    use taffy::Display;

    fn scales() -> Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn doc() -> (LayerTree, LayerId) {
        let mut layers = LayerTree::new();
        let id = match layers.add_pixel_layer(
            "a",
            Rect {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            },
            None,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        (layers, id)
    }

    fn built(width: f32, height: f32) -> (crate::Workspace, LayerControls) {
        let scales = scales();
        let mut workspace = build_workspace(&scales);
        let controls = match insert_layer_controls(&mut workspace.tree, workspace.layers, &scales) {
            Ok(controls) => controls,
            Err(err) => unreachable!("{err:?}"),
        };
        workspace.tree.compute_layout(width, height);
        (workspace, controls)
    }

    fn slider_value(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> (f64, bool) {
        match tree.payload(id) {
            Some(WidgetKind::Slider(state)) => (state.value, state.disabled),
            other => unreachable!("expected Slider, got {other:?}"),
        }
    }

    fn dropdown_selected(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> (Option<usize>, bool) {
        match tree.payload(id) {
            Some(WidgetKind::Dropdown(state)) => (state.selected(), state.is_disabled()),
            other => unreachable!("expected Dropdown, got {other:?}"),
        }
    }

    fn checkbox_checked(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> (Toggled, bool) {
        match tree.payload(id) {
            Some(WidgetKind::Checkbox(state)) => (state.checked, state.disabled),
            other => unreachable!("expected Checkbox, got {other:?}"),
        }
    }

    #[test]
    fn every_blend_mode_has_a_distinct_non_empty_label_in_all_order() {
        let options = blend_mode_options();
        assert_eq!(options.len(), BlendMode::ALL.len());
        let distinct: HashSet<&str> = options.iter().map(String::as_str).collect();
        assert_eq!(
            distinct.len(),
            BlendMode::ALL.len(),
            "labels must be distinct"
        );
        for (index, &mode) in BlendMode::ALL.iter().enumerate() {
            assert!(!blend_mode_label(mode).trim().is_empty());
            assert_eq!(
                options.get(index).map(String::as_str),
                Some(blend_mode_label(mode))
            );
            assert_eq!(blend_mode_index(mode), Some(index));
        }
        assert_eq!(
            blend_mode_label(BlendMode::LinearDodge),
            "Linear Dodge (Add)"
        );
        assert_eq!(blend_mode_label(BlendMode::ColorBurn), "Color Burn");
    }

    #[test]
    fn the_strip_survives_repopulating_the_layers_panel() {
        let scales = scales();
        let (mut ws, controls) = built(1600.0, 900.0);
        let tree = &mut ws.tree;
        let (layers, _) = doc();
        for _ in 0..3 {
            if let Err(err) = populate_layers_panel(tree, ws.layers, &scales, &layers) {
                unreachable!("{err:?}");
            }
        }
        for id in [
            controls.root,
            controls.opacity,
            controls.blend,
            controls.visible,
        ] {
            assert!(
                tree.contains(id),
                "populate_layers_panel must not remove the strip"
            );
            assert!(
                !tree.is_within(ws.layers.body, id),
                "the strip is outside the body"
            );
            assert!(tree.is_within(ws.layers.root, id));
        }
    }

    fn assert_hittable(tree: &WidgetTree<WidgetKind>, controls: &LayerControls) {
        for id in [controls.opacity, controls.blend, controls.visible] {
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
    fn the_three_controls_are_hittable_at_the_default_and_the_narrowest_rail() {
        let scales = scales();
        let (mut ws, controls) = built(1600.0, 900.0);
        let tree = &mut ws.tree;
        let (layers, _) = doc();
        if let Err(err) = populate_layers_panel(tree, ws.layers, &scales, &layers) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(1600.0, 900.0);
        assert_hittable(tree, &controls);

        if let Err(err) = set_rail_width(tree, ws.rail, ws.divider, 150.0) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(1600.0, 900.0);
        assert_hittable(tree, &controls);
    }

    #[test]
    fn collapsing_the_layers_panel_hides_the_strip() {
        let (mut ws, controls) = built(1600.0, 900.0);
        let tree = &mut ws.tree;
        // The slider's centre while the panel is expanded (review
        // RT135-2): after collapsing, nothing of the strip may be there.
        let Some(open) = tree.bounds(controls.opacity) else {
            unreachable!("laid out");
        };
        #[allow(clippy::cast_precision_loss)]
        let former = (
            (open.x + i64::from(open.width / 2)) as f32,
            (open.y + i64::from(open.height / 2)) as f32,
        );
        assert!(
            tree.hit_test(former)
                .is_some_and(|hit| tree.is_within(controls.root, hit)),
            "precondition: the expanded strip is under its slider's centre"
        );
        if let Err(err) = crate::set_panel_collapsed(tree, ws.layers, true) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(1600.0, 900.0);
        assert_eq!(
            tree.style(controls.root).map(|style| style.display),
            Some(Display::None),
            "collapsing the panel hides the strip itself"
        );
        let hit = tree.hit_test(former);
        assert!(
            !hit.is_some_and(|hit| tree.is_within(controls.root, hit)),
            "the slider's former centre now hits {hit:?}, inside the hidden strip"
        );
        let Some(bounds) = tree.bounds(controls.opacity) else {
            unreachable!("still in the tree");
        };
        #[allow(clippy::cast_precision_loss)]
        let hit = tree.hit_test((
            (bounds.x + i64::from(bounds.width / 2)) as f32,
            (bounds.y + i64::from(bounds.height / 2)) as f32,
        ));
        assert!(
            bounds.width == 0 || bounds.height == 0 || hit != Some(controls.opacity),
            "a collapsed panel's controls must not be hittable: {bounds:?}"
        );
        if let Err(err) = crate::set_panel_collapsed(tree, ws.layers, false) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(1600.0, 900.0);
        assert_ne!(
            tree.style(controls.root).map(|style| style.display),
            Some(Display::None),
            "expanding shows the strip again"
        );
        assert_hittable(tree, &controls);
    }

    #[test]
    fn sync_reflects_opacity_blend_mode_and_visibility() {
        let (mut ws, controls) = built(1600.0, 900.0);
        let tree = &mut ws.tree;
        let (mut layers, id) = doc();
        let results = [
            layers.set_opacity(id, 0.4),
            layers.set_blend_mode(id, BlendMode::Multiply),
            layers.set_visible(id, false),
        ];
        assert!(results.iter().all(Result::is_ok));
        if let Err(err) = sync_layer_controls(tree, controls, &layers, Some(id), None) {
            unreachable!("{err:?}");
        }
        let (value, disabled) = slider_value(tree, controls.opacity);
        assert!((value - 40.0).abs() < 1e-4, "got {value}");
        assert!(!disabled);
        assert_eq!(
            dropdown_selected(tree, controls.blend),
            (blend_mode_index(BlendMode::Multiply), false)
        );
        assert_eq!(
            checkbox_checked(tree, controls.visible),
            (Toggled::False, false)
        );
        assert!(
            matches!(
                sync_layer_controls(tree, controls, &layers, Some(id), None),
                Ok(false)
            ),
            "a second sync with nothing changed must touch nothing"
        );
    }

    #[test]
    fn sync_with_no_active_layer_disables_all_three_and_some_re_enables() {
        let (mut ws, controls) = built(1600.0, 900.0);
        let tree = &mut ws.tree;
        let (layers, id) = doc();
        if let Err(err) = sync_layer_controls(tree, controls, &layers, Some(id), None) {
            unreachable!("{err:?}");
        }
        if let Err(err) = sync_layer_controls(tree, controls, &layers, None, None) {
            unreachable!("{err:?}");
        }
        assert!(slider_value(tree, controls.opacity).1);
        assert!(dropdown_selected(tree, controls.blend).1);
        assert!(checkbox_checked(tree, controls.visible).1);
        if let Err(err) = sync_layer_controls(tree, controls, &layers, Some(id), None) {
            unreachable!("{err:?}");
        }
        assert!(!slider_value(tree, controls.opacity).1);
        assert!(!dropdown_selected(tree, controls.blend).1);
        assert!(!checkbox_checked(tree, controls.visible).1);
    }

    #[test]
    fn sync_skips_the_opacity_slider_while_it_holds_capture() {
        let (mut ws, controls) = built(1600.0, 900.0);
        let tree = &mut ws.tree;
        let (mut layers, id) = doc();
        if let Err(err) = sync_layer_controls(tree, controls, &layers, Some(id), None) {
            unreachable!("{err:?}");
        }
        if let Err(err) = aurora_widgets::widgets::set_slider_value(tree, controls.opacity, 25.0) {
            unreachable!("{err:?}");
        }
        assert!(layers.set_blend_mode(id, BlendMode::Screen).is_ok());
        if let Err(err) =
            sync_layer_controls(tree, controls, &layers, Some(id), Some(controls.opacity))
        {
            unreachable!("{err:?}");
        }
        let (value, _) = slider_value(tree, controls.opacity);
        assert!(
            (value - 25.0).abs() < 1e-9,
            "a captured slider keeps its own value, got {value}"
        );
        assert_eq!(
            dropdown_selected(tree, controls.blend).0,
            blend_mode_index(BlendMode::Screen),
            "the other controls still sync"
        );
        if let Err(err) = sync_layer_controls(tree, controls, &layers, Some(id), None) {
            unreachable!("{err:?}");
        }
        let (value, _) = slider_value(tree, controls.opacity);
        assert!(
            (value - 100.0).abs() < 1e-9,
            "uncaptured, the document wins, got {value}"
        );
    }
}
