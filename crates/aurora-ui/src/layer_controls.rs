//! The Layers panel's **editable** controls: a blend-mode dropdown, an
//! Opacity slider (0.135.0) and a Fill slider (0.195.0), each with a live
//! "100 %" readout, for the *active* layer — widgets that edit a live
//! document. PLAN.md M1.8, "Layers, history, tool-options panels".
//!
//! **Where they live (0.195.0: above the list).** [`crate::populate_layers_panel`]
//! clears the whole of `panel.body` on every call (it is the only way layer
//! rows are built), so anything inside the body would be destroyed and
//! re-created with fresh `WidgetId`s on every structural change —
//! including mid-drag. The strip is therefore a child of the panel's own
//! *root*, moved (`WidgetTree::move_child`) to sit right after the title
//! row and before the list's viewport, as in Photoshop; it survives every
//! repopulation, and [`crate::set_panel_collapsed`] hides it with the body.
//! It never shrinks (the list takes the squeeze). The old visibility
//! checkbox is gone: every row's eye replaces it.
//!
//! **Sliders, not numeric fields.** The reference uses `[100 %]` fields.
//! `aurora-widgets` has no validated numeric field (its `TextField` is a
//! free-text editor with no per-field parse-and-commit route in the app),
//! so Opacity and Fill are sliders with a "100 %" readout label; the drag
//! is one undo step. **Fill is not applied to a Pass Through group** (it
//! is reported, design decision 3), so its slider is disabled there and
//! its row says why in its accessible description.
//!
//! **This module knows nothing about undo.** It builds the controls,
//! mirrors a document's state into them ([`sync_layer_controls`]), and
//! names the blend modes ([`blend_mode_label`]). Turning a widget outcome
//! into a document edit — live drag, one undo step per gesture — is
//! `aurora-app`'s job, the same split the gallery panel draws.

use aurora_doc::{BlendMode, LayerId, LayerTree};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::{TaffyZero as _, length};
use taffy::{FlexDirection, Rect as LayoutRect, Size, Style};

use crate::panel::PanelHandle;

/// The controls, plus the strip that holds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerControls {
    /// The strip container — a child of the Layers panel's root.
    pub root: WidgetId,
    /// Opacity, in percent (`0..=100`).
    pub opacity: WidgetId,
    /// Fill, in percent (`0..=100`); disabled for a Pass Through group.
    pub fill: WidgetId,
    /// The Opacity slider's "100 %" readout.
    pub opacity_readout: WidgetId,
    /// The Fill slider's "100 %" readout.
    pub fill_readout: WidgetId,
    /// The Fill row (its accessible description carries the reason when
    /// Fill is disabled).
    pub fill_row: WidgetId,
    /// Blend mode; option `i` is `BlendMode::modes_for(is_group)[i]` for
    /// the active layer (Pass Through first for a group, 0.179.0).
    pub blend: WidgetId,
}

/// The blend-mode dropdown's options for a pixel or adjustment layer:
/// the 27 layer modes in `BlendMode::ALL` order, without the group-only
/// Pass Through. See [`blend_mode_options_for`].
#[must_use]
pub fn blend_mode_options() -> Vec<String> {
    blend_mode_options_for(false)
}

/// The blend-mode dropdown's options for a group (`is_group`, Pass
/// Through first, as in Photoshop's own group menu, 0.179.0) or for any
/// other layer — `BlendMode::modes_for`, turned into labels in this one
/// place, so an option index and the mode it names cannot drift apart.
#[must_use]
pub fn blend_mode_options_for(is_group: bool) -> Vec<String> {
    BlendMode::modes_for(is_group)
        .iter()
        .map(|&mode| blend_mode_label(mode).to_owned())
        .collect()
}

/// The dropdown option index of `mode` in a pixel or adjustment layer's
/// list ([`blend_mode_options`]); `None` for Pass Through.
#[must_use]
pub fn blend_mode_index(mode: BlendMode) -> Option<usize> {
    blend_mode_index_for(false, mode)
}

/// The dropdown option index of `mode` in [`blend_mode_options_for`]'s
/// list for `is_group`.
#[must_use]
pub fn blend_mode_index_for(is_group: bool, mode: BlendMode) -> Option<usize> {
    BlendMode::modes_for(is_group)
        .iter()
        .position(|&candidate| candidate == mode)
}

/// The mode the blend dropdown's option `index` names while `id` is the
/// active layer — its own list ([`blend_mode_options_for`]) decides, so
/// Pass Through can only ever be picked for a group.
#[must_use]
pub fn blend_mode_at(layers: &LayerTree, id: LayerId, index: usize) -> Option<BlendMode> {
    let is_group = layers.kind(id)?.is_group();
    BlendMode::modes_for(is_group).get(index).copied()
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
        BlendMode::PassThrough => "Pass Through",
    }
}

/// The fixed width of a control row's caption ("Opacity"): `spacing.xxl`
/// plus `spacing.xs`.
fn caption_width(scales: &Scales) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let width = (scales.spacing.xxl + scales.spacing.xs) as f32;
    width
}

/// The fixed width of a readout ("100 %"): `spacing.xxl`.
fn readout_width(scales: &Scales) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let width = scales.spacing.xxl as f32;
    width
}

/// The strip's rows: the blend dropdown, Opacity and Fill.
const CONTROL_ROWS: f32 = 3.0;

fn strip_style(scales: &Scales) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let pad = length(scales.spacing.sm as f32);
    #[allow(clippy::cast_precision_loss)]
    let gap = length(scales.spacing.xs as f32);
    let row = widgets::row_height(scales);
    #[allow(clippy::cast_precision_loss)]
    let chrome = 2.0 * scales.spacing.sm as f32 + 2.0 * scales.spacing.xs as f32;
    Style {
        flex_direction: FlexDirection::Column,
        // Above the list (0.195.0): never shrunk; the list's viewport
        // takes the squeeze and scrolls. Its floor is its own content.
        flex_shrink: 0.0,
        min_size: Size {
            width: taffy::Dimension::ZERO,
            height: length(CONTROL_ROWS * row + chrome),
        },
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

fn control_row_style(scales: &Scales) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let gap = length(scales.spacing.xs as f32);
    Style {
        flex_direction: FlexDirection::Row,
        align_items: Some(taffy::AlignItems::CENTER),
        flex_shrink: 0.0,
        gap: Size {
            width: gap,
            height: gap,
        },
        ..Default::default()
    }
}

/// Adds the controls strip to `panel` (the Layers panel) as a child of
/// its root, **between the title row and the list** (see this module's
/// doc comment). The controls start disabled, showing `Normal`, `100 %`
/// and `100 %`; call [`sync_layer_controls`] to show a real layer.
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
    let built = tree
        .move_child(root, panel.root, 1)
        .and_then(|()| build(tree, root, scales))
        .and_then(|controls| crate::panel::refresh_panel_floor(tree, panel).map(|()| controls));
    if built.is_err() {
        let _ = tree.remove(root);
    }
    built
}

/// One captioned slider row: `[caption | slider | readout]`.
fn slider_row(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    scales: &Scales,
    caption: &str,
) -> Result<(WidgetId, WidgetId, WidgetId), WidgetError> {
    let mut node = accesskit::Node::new(accesskit::Role::Group);
    node.set_label(caption.to_owned());
    let row = tree.insert(root, control_row_style(scales), node, WidgetKind::Container)?;
    let label = widgets::insert_label(tree, row, scales, caption)?;
    let mut label_style = tree.style(label).cloned().unwrap_or_default();
    label_style.size.width = length(caption_width(scales));
    tree.set_style(label, label_style)?;
    let slider = widgets::insert_slider(tree, row, scales, caption, 100.0, 0.0, 100.0)?;
    let readout = widgets::insert_label(tree, row, scales, percent_text(100.0))?;
    let mut readout_style = tree.style(readout).cloned().unwrap_or_default();
    readout_style.size.width = length(readout_width(scales));
    tree.set_style(readout, readout_style)?;
    Ok((row, slider, readout))
}

/// `"100 %"`.
fn percent_text(percent: f64) -> String {
    format!("{percent:.0} %")
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
    let (_, opacity, opacity_readout) = slider_row(tree, root, scales, "Opacity")?;
    let (fill_row, fill, fill_readout) = slider_row(tree, root, scales, "Fill")?;
    let controls = LayerControls {
        root,
        opacity,
        fill,
        opacity_readout,
        fill_readout,
        fill_row,
        blend,
    };
    set_disabled(tree, controls, true, false)?;
    Ok(controls)
}

/// Why Fill is off for a Pass Through group (its row's description).
const FILL_PASS_THROUGH_REASON: &str =
    "Fill is not applied to a Pass Through group; choose another blend mode to use it";

fn set_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    controls: LayerControls,
    disabled: bool,
    fill_unavailable: bool,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    let fill_disabled = disabled || fill_unavailable;
    for (slider, off) in [(controls.opacity, disabled), (controls.fill, fill_disabled)] {
        if let Some(WidgetKind::Slider(state)) = tree.payload(slider)
            && state.disabled != off
        {
            widgets::set_slider_disabled(tree, slider, off)?;
            changed = true;
        }
    }
    for (readout, off) in [
        (controls.opacity_readout, disabled),
        (controls.fill_readout, fill_disabled),
    ] {
        changed |= widgets::set_label_disabled(tree, readout, off)?;
    }
    if widgets::dropdown_state(tree, controls.blend)?.is_disabled() != disabled {
        widgets::set_dropdown_disabled(tree, controls.blend, disabled)?;
        changed = true;
    }
    // The reason travels on the Fill row, so a screen reader that lands on
    // the dimmed control hears why.
    let wanted = (fill_unavailable && !disabled).then_some(FILL_PASS_THROUGH_REASON);
    let current = tree
        .accessibility(controls.fill_row)
        .and_then(|node| node.description().map(str::to_owned));
    if current.as_deref() != wanted {
        let mut node = accesskit::Node::new(accesskit::Role::Group);
        node.set_label("Fill".to_owned());
        if let Some(reason) = wanted {
            node.set_description(reason.to_owned());
        }
        tree.set_accessibility(controls.fill_row, node)?;
        changed = true;
    }
    Ok(changed)
}

/// Mirrors one slider and its readout from `target` percent, unless the
/// slider is `captured` (a drag is the source of truth).
fn sync_slider(
    tree: &mut WidgetTree<WidgetKind>,
    slider: WidgetId,
    readout: WidgetId,
    target: f64,
    captured: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    if captured == Some(slider) {
        return Ok(false);
    }
    let mut changed = false;
    let shown = match tree.payload(slider) {
        Some(WidgetKind::Slider(state)) => state.value,
        _ => return Err(WidgetError::WrongWidgetKind(slider)),
    };
    if shown.to_bits() != target.to_bits() {
        widgets::set_slider_value(tree, slider, target)?;
        changed = true;
    }
    changed |= widgets::set_label_text(tree, readout, &percent_text(target))?;
    Ok(changed)
}

/// Updates the readouts of a slider being dragged (the document follows
/// the slider, so only the text needs to follow the slider).
///
/// # Errors
///
/// Returns whatever the label setter returns for a wrong widget.
pub fn sync_layer_readouts(
    tree: &mut WidgetTree<WidgetKind>,
    controls: LayerControls,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    for (slider, readout) in [
        (controls.opacity, controls.opacity_readout),
        (controls.fill, controls.fill_readout),
    ] {
        if let Some(WidgetKind::Slider(state)) = tree.payload(slider) {
            let text = percent_text(state.value);
            changed |= widgets::set_label_text(tree, readout, &text)?;
        }
    }
    Ok(changed)
}

/// Mirrors `active`'s opacity, fill and blend mode into `controls`, in
/// place — never inserting or removing a widget, so the ids a caller
/// holds (and a pointer capture on a slider) stay valid. Returns whether
/// anything changed; a control already showing the right state is not
/// touched at all, so this is cheap enough for a caller to run on every
/// event-loop iteration as a catch-all.
///
/// `None`, or an id `layers` doesn't know, disables everything (their
/// shown values are left as they were). `Some` re-enables them *first*,
/// since `set_slider_value` refuses a disabled slider; Fill stays
/// disabled while `active` is a Pass Through group (not applied there).
///
/// `captured` is the widget currently holding pointer capture: when it
/// is a slider, that slider's value is **not** overwritten — mid-drag it
/// is the source of truth and the document follows it, and writing the
/// document's value back would fight the pointer. Its readout follows it.
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
        return set_disabled(tree, controls, true, false);
    };
    let is_group = layers.kind(id).is_some_and(aurora_doc::LayerKind::is_group);
    let mode = layers.blend_mode(id).unwrap_or_default();
    let fill_unavailable = is_group && mode == BlendMode::PassThrough;
    let mut changed = set_disabled(tree, controls, false, fill_unavailable)?;
    changed |= sync_slider(
        tree,
        controls.opacity,
        controls.opacity_readout,
        f64::from(layers.opacity(id).unwrap_or(1.0)) * 100.0,
        captured,
    )?;
    changed |= sync_slider(
        tree,
        controls.fill,
        controls.fill_readout,
        f64::from(layers.fill_opacity(id).unwrap_or(1.0)) * 100.0,
        captured,
    )?;
    if captured.is_some() {
        changed |= sync_layer_readouts(tree, controls)?;
    }
    // A group lists Pass Through first and anything else does not list it
    // at all (0.179.0), so the options follow the active layer's kind.
    let index = blend_mode_index_for(is_group, mode);
    let options = blend_mode_options_for(is_group);
    let shown = widgets::dropdown_state(tree, controls.blend)?;
    if shown.options() != options.as_slice() {
        widgets::set_dropdown_options(tree, controls.blend, options, index)?;
        changed = true;
    } else if shown.selected() != index {
        widgets::set_dropdown_selected(tree, controls.blend, index)?;
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
        LayerControls, blend_mode_index, blend_mode_index_for, blend_mode_label,
        blend_mode_options, blend_mode_options_for, insert_layer_controls, sync_layer_controls,
    };
    use crate::layers_panel::populate_layers_panel;
    use crate::workspace::{build_workspace, set_rail_width};
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

    fn label_text(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> String {
        match tree.payload(id) {
            Some(WidgetKind::Label(state)) => state.text.clone(),
            other => unreachable!("expected Label, got {other:?}"),
        }
    }

    #[test]
    fn every_blend_mode_has_a_distinct_non_empty_label_in_all_order() {
        let options = blend_mode_options_for(true);
        assert_eq!(options.len(), BlendMode::ALL.len());
        assert_eq!(blend_mode_options().len(), BlendMode::ALL.len() - 1);
        let distinct: HashSet<&str> = options.iter().map(String::as_str).collect();
        assert_eq!(
            distinct.len(),
            BlendMode::ALL.len(),
            "labels must be distinct"
        );
        for (index, &mode) in BlendMode::modes_for(true).iter().enumerate() {
            assert!(!blend_mode_label(mode).trim().is_empty());
            assert_eq!(
                options.get(index).map(String::as_str),
                Some(blend_mode_label(mode))
            );
            assert_eq!(blend_mode_index_for(true, mode), Some(index));
        }
        for (index, &mode) in BlendMode::modes_for(false).iter().enumerate() {
            assert_eq!(blend_mode_index(mode), Some(index));
        }
        assert_eq!(blend_mode_index(BlendMode::PassThrough), None);
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
            controls.fill,
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
        for id in [controls.opacity, controls.blend, controls.fill] {
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
    fn sync_reflects_opacity_blend_mode_and_fill() {
        let (mut ws, controls) = built(1600.0, 900.0);
        let tree = &mut ws.tree;
        let (mut layers, id) = doc();
        let results = [
            layers.set_opacity(id, 0.4),
            layers.set_blend_mode(id, BlendMode::Multiply),
            layers.set_fill_opacity(id, 0.25),
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
        let (fill, fill_disabled) = slider_value(tree, controls.fill);
        assert!((fill - 25.0).abs() < 1e-4, "got {fill}");
        assert!(!fill_disabled);
        assert_eq!(label_text(tree, controls.opacity_readout), "40 %");
        assert_eq!(label_text(tree, controls.fill_readout), "25 %");
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
        assert!(slider_value(tree, controls.fill).1);
        if let Err(err) = sync_layer_controls(tree, controls, &layers, Some(id), None) {
            unreachable!("{err:?}");
        }
        assert!(!slider_value(tree, controls.opacity).1);
        assert!(!dropdown_selected(tree, controls.blend).1);
        assert!(!slider_value(tree, controls.fill).1);
    }

    #[test]
    fn the_strip_sits_between_the_title_and_the_list_in_the_panel_order() {
        let (ws, controls) = built(1600.0, 900.0);
        let children = ws.tree.children(ws.layers.root).unwrap_or_default();
        assert_eq!(
            children,
            &[ws.layers.header, controls.root, ws.layers.viewport],
            "title, controls, list"
        );
        let labels: Vec<Option<String>> = [controls.opacity, controls.fill]
            .iter()
            .map(|&id| {
                ws.tree
                    .accessibility(id)
                    .and_then(|n| n.label().map(str::to_owned))
            })
            .collect();
        assert_eq!(
            labels,
            [Some("Opacity".to_owned()), Some("Fill".to_owned())]
        );
    }

    #[test]
    fn fill_follows_the_layer_and_is_disabled_with_a_reason_for_a_pass_through_group() {
        let (mut ws, controls) = built(1600.0, 900.0);
        let tree = &mut ws.tree;
        let (mut layers, pixel) = doc();
        let group = match layers.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let reason = |tree: &WidgetTree<WidgetKind>| {
            tree.accessibility(controls.fill_row)
                .and_then(|n| n.description().map(str::to_owned))
        };
        // Pass Through: Fill off, with the reason.
        assert!(layers.set_blend_mode(group, BlendMode::PassThrough).is_ok());
        assert!(sync_layer_controls(tree, controls, &layers, Some(group), None).is_ok());
        assert!(slider_value(tree, controls.fill).1);
        assert!(reason(tree).is_some_and(|text| text.contains("Pass Through")));
        // An isolated group: Fill on, no reason.
        assert!(layers.set_blend_mode(group, BlendMode::Normal).is_ok());
        assert!(sync_layer_controls(tree, controls, &layers, Some(group), None).is_ok());
        assert!(!slider_value(tree, controls.fill).1);
        assert_eq!(reason(tree), None);
        // A pixel layer: Fill on.
        assert!(sync_layer_controls(tree, controls, &layers, Some(pixel), None).is_ok());
        assert!(!slider_value(tree, controls.fill).1);
    }

    /// AC-5 (0.179.0): a group's dropdown lists Pass Through first and
    /// shows it selected; a pixel layer's lists it nowhere, and switching
    /// the active layer between the two swaps the list.
    #[test]
    fn the_dropdown_lists_pass_through_first_for_a_group_and_never_for_a_layer() {
        let (mut ws, controls) = built(1600.0, 900.0);
        let tree = &mut ws.tree;
        let (mut layers, pixel) = doc();
        let group = match layers.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(layers.set_blend_mode(group, BlendMode::PassThrough).is_ok());
        assert!(sync_layer_controls(tree, controls, &layers, Some(group), None).is_ok());
        let Ok(state) = aurora_widgets::widgets::dropdown_state(tree, controls.blend) else {
            unreachable!("dropdown");
        };
        assert_eq!(
            state.options().first().map(String::as_str),
            Some("Pass Through")
        );
        assert_eq!(state.options().len(), BlendMode::ALL.len());
        assert_eq!(state.selected(), Some(0));
        assert_eq!(
            super::blend_mode_at(&layers, group, 0),
            Some(BlendMode::PassThrough)
        );
        assert_eq!(
            super::blend_mode_at(&layers, group, 1),
            Some(BlendMode::Normal)
        );

        assert!(sync_layer_controls(tree, controls, &layers, Some(pixel), None).is_ok());
        let Ok(state) = aurora_widgets::widgets::dropdown_state(tree, controls.blend) else {
            unreachable!("dropdown");
        };
        assert!(!state.options().iter().any(|o| o == "Pass Through"));
        assert_eq!(state.options().len(), BlendMode::ALL.len() - 1);
        assert_eq!(state.selected(), blend_mode_index(BlendMode::Normal));
        assert_eq!(
            super::blend_mode_at(&layers, pixel, 0),
            Some(BlendMode::Normal)
        );
        assert!(
            BlendMode::modes_for(false)
                .iter()
                .enumerate()
                .all(|(i, &m)| super::blend_mode_at(&layers, pixel, i) == Some(m))
        );
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
