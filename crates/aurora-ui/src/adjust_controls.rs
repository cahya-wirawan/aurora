//! The Properties panel's **Vibrance, Exposure and Photo Filter editors**
//! (0.239.0): one hidden column holding three groups of sliders with a
//! readout each (and Photo Filter's Preserve Luminosity checkbox), of
//! which at most one is shown, the one matching the active layer.
//!
//! **Where it lives.** Inside the Properties panel's tool-controls strip
//! beside the Hue/Saturation editor ([`crate::insert_tool_controls`]), so
//! every pointer, key and accessibility event on it reaches `aurora-app`
//! through the same `ToolControls` routing the other editors use. It
//! starts hidden (`Display::None`) and disabled.
//!
//! **This module knows nothing about undo or pixels**: it builds the
//! widgets, maps a slider to a field of an [`Adjustment`]
//! ([`AdjustSlider::with_value`]) and mirrors a document into the widgets
//! ([`sync_adjust_controls`]). One undo step per drag and per checkbox
//! toggle is `aurora-app`'s job (`adjust_app`).
//!
//! **Units.** Vibrance and Saturation in whole percent; Exposure in stops
//! to two decimals, Offset to three, Gamma to two (Photoshop's fields);
//! Density in whole percent; the Photo Filter colour as three 8-bit
//! channel sliders (`0..=255`), because the properties panel has no
//! colour picker. The presets (`Warming (85)` and the others) are palette
//! commands, not widgets here.

use aurora_core::{
    EXPOSURE_MAX, ExposureParams, GAMMA_MAX, GAMMA_MIN, OFFSET_MAX, PhotoFilterParams,
    VibranceParams,
};
use aurora_doc::{Adjustment, LayerId, LayerTree};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{Display, FlexDirection, Size, Style};

/// Which of the three adjustments an editor group belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdjustKind {
    Vibrance,
    Exposure,
    PhotoFilter,
}

impl AdjustKind {
    /// Every kind, in group order.
    pub const ALL: [Self; 3] = [Self::Vibrance, Self::Exposure, Self::PhotoFilter];

    /// Position in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Vibrance => 0,
            Self::Exposure => 1,
            Self::PhotoFilter => 2,
        }
    }

    /// The kind of adjustment `adjustment` is, if it is one of these.
    #[must_use]
    pub const fn of(adjustment: &Adjustment) -> Option<Self> {
        match adjustment {
            Adjustment::Vibrance(_) => Some(Self::Vibrance),
            Adjustment::Exposure(_) => Some(Self::Exposure),
            Adjustment::PhotoFilter(_) => Some(Self::PhotoFilter),
            _ => None,
        }
    }
}

/// One of the nine sliders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdjustSlider {
    /// Vibrance, `-100..=100`.
    Vibrance,
    /// The Vibrance layer's saturation, `-100..=100`.
    VibranceSaturation,
    /// Exposure in stops, `-20..=20`.
    Exposure,
    /// Offset, `-0.5..=0.5`.
    Offset,
    /// Gamma correction, `0.10..=9.99`.
    Gamma,
    /// Photo Filter density, `1..=100`.
    Density,
    /// The filter colour's red level, `0..=255`.
    Red,
    /// The filter colour's green level, `0..=255`.
    Green,
    /// The filter colour's blue level, `0..=255`.
    Blue,
}

impl AdjustSlider {
    /// Every slider, in layout order — the one place that order is fixed.
    pub const ALL: [Self; 9] = [
        Self::Vibrance,
        Self::VibranceSaturation,
        Self::Exposure,
        Self::Offset,
        Self::Gamma,
        Self::Density,
        Self::Red,
        Self::Green,
        Self::Blue,
    ];

    /// Position in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Vibrance => 0,
            Self::VibranceSaturation => 1,
            Self::Exposure => 2,
            Self::Offset => 3,
            Self::Gamma => 4,
            Self::Density => 5,
            Self::Red => 6,
            Self::Green => 7,
            Self::Blue => 8,
        }
    }

    /// The group the slider belongs to.
    #[must_use]
    pub const fn kind(self) -> AdjustKind {
        match self {
            Self::Vibrance | Self::VibranceSaturation => AdjustKind::Vibrance,
            Self::Exposure | Self::Offset | Self::Gamma => AdjustKind::Exposure,
            Self::Density | Self::Red | Self::Green | Self::Blue => AdjustKind::PhotoFilter,
        }
    }

    /// The slider's accessible name (unique among the editors' sliders).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Vibrance => "Vibrance",
            Self::VibranceSaturation => "Vibrance saturation",
            Self::Exposure => "Exposure",
            Self::Offset => "Exposure offset",
            Self::Gamma => "Exposure gamma",
            Self::Density => "Filter density",
            Self::Red => "Filter red",
            Self::Green => "Filter green",
            Self::Blue => "Filter blue",
        }
    }

    /// The readout's prefix.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Vibrance => "Vibrance",
            Self::VibranceSaturation => "Saturation",
            Self::Exposure => "Exposure",
            Self::Offset => "Offset",
            Self::Gamma => "Gamma",
            Self::Density => "Density",
            Self::Red => "Red",
            Self::Green => "Green",
            Self::Blue => "Blue",
        }
    }

    /// The slider's `(min, max)`.
    #[must_use]
    pub const fn range(self) -> (f64, f64) {
        match self {
            Self::Vibrance | Self::VibranceSaturation => (-100.0, 100.0),
            Self::Exposure => (-(EXPOSURE_MAX as f64), EXPOSURE_MAX as f64),
            Self::Offset => (-(OFFSET_MAX as f64), OFFSET_MAX as f64),
            Self::Gamma => (GAMMA_MIN as f64, GAMMA_MAX as f64),
            Self::Density => (1.0, 100.0),
            Self::Red | Self::Green | Self::Blue => (0.0, 255.0),
        }
    }

    /// Decimal places the slider's value is kept to (and its readout shows).
    #[must_use]
    pub const fn decimals(self) -> usize {
        match self {
            Self::Exposure | Self::Gamma => 2,
            Self::Offset => 3,
            _ => 0,
        }
    }

    /// The readout text for `value`: `+1.50` style for Exposure, plain
    /// otherwise.
    #[must_use]
    pub fn readout(self, value: f64) -> String {
        let places = self.decimals();
        if self == Self::Exposure {
            format!("{}: {value:+.places$}", self.short_label())
        } else {
            format!("{}: {value:.places$}", self.short_label())
        }
    }

    /// The slider position for `adjustment`, `None` when it is not this
    /// slider's adjustment.
    #[must_use]
    pub fn value_of(self, adjustment: &Adjustment) -> Option<f64> {
        let channel = |i: usize| {
            adjustment
                .photo_filter()
                .and_then(|p| p.colour().get(i).copied())
                .map(|c| f64::from((c * 255.0).round()))
        };
        match self {
            Self::Vibrance => adjustment.vibrance().map(|p| f64::from(p.vibrance())),
            Self::VibranceSaturation => adjustment.vibrance().map(|p| f64::from(p.saturation())),
            Self::Exposure => adjustment.exposure().map(|p| f64::from(p.exposure())),
            Self::Offset => adjustment.exposure().map(|p| f64::from(p.offset())),
            Self::Gamma => adjustment.exposure().map(|p| f64::from(p.gamma())),
            Self::Density => adjustment.photo_filter().map(|p| f64::from(p.density())),
            Self::Red => channel(0),
            Self::Green => channel(1),
            Self::Blue => channel(2),
        }
    }

    /// `value` rounded to the slider's decimals and clamped to its range.
    #[must_use]
    pub fn snap(self, value: f64) -> f64 {
        let (min, max) = self.range();
        let scale = 10_f64.powi(i32::try_from(self.decimals()).unwrap_or(0));
        ((value * scale).round() / scale).clamp(min, max)
    }

    /// `adjustment` with this slider moved to `value` (snapped), `None`
    /// for a non-finite `value` or an adjustment of another kind.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn with_value(self, adjustment: &Adjustment, value: f64) -> Option<Adjustment> {
        if !value.is_finite() {
            return None;
        }
        let v = self.snap(value) as f32;
        match self {
            Self::Vibrance | Self::VibranceSaturation => {
                let p: VibranceParams = adjustment.vibrance()?;
                let next = if self == Self::Vibrance {
                    p.with_vibrance(v)
                } else {
                    p.with_saturation(v)
                };
                next.ok().map(Adjustment::Vibrance)
            }
            Self::Exposure | Self::Offset | Self::Gamma => {
                let p: ExposureParams = adjustment.exposure()?;
                let next = match self {
                    Self::Exposure => p.with_exposure(v),
                    Self::Offset => p.with_offset(v),
                    _ => p.with_gamma(v),
                };
                next.ok().map(Adjustment::Exposure)
            }
            Self::Density => adjustment
                .photo_filter()?
                .with_density(v)
                .ok()
                .map(Adjustment::PhotoFilter),
            Self::Red | Self::Green | Self::Blue => {
                let p: PhotoFilterParams = adjustment.photo_filter()?;
                let mut colour = p.colour();
                let slot = colour.get_mut(match self {
                    Self::Red => 0,
                    Self::Green => 1,
                    _ => 2,
                })?;
                *slot = v / 255.0;
                p.with_colour(colour).ok().map(Adjustment::PhotoFilter)
            }
        }
    }
}

/// `id`'s adjustment when it is Vibrance, Exposure or Photo Filter.
#[must_use]
pub fn adjust_of(layers: &LayerTree, id: LayerId) -> Option<&Adjustment> {
    layers
        .adjustment(id)
        .filter(|adjustment| AdjustKind::of(adjustment).is_some())
}

/// The nine sliders, their readouts, Photo Filter's checkbox and the
/// containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdjustControls {
    /// The column container.
    pub root: WidgetId,
    /// The per-kind groups, indexed by [`AdjustKind::index`].
    pub groups: [WidgetId; 3],
    /// The sliders, indexed by [`AdjustSlider::index`].
    pub sliders: [WidgetId; 9],
    /// Their readout labels, same order.
    pub readouts: [WidgetId; 9],
    /// Photo Filter's "Preserve Luminosity" checkbox.
    pub preserve: WidgetId,
}

impl AdjustControls {
    /// Which slider `id` is, if any.
    #[must_use]
    pub fn slider_of(&self, id: WidgetId) -> Option<AdjustSlider> {
        AdjustSlider::ALL
            .into_iter()
            .find(|slider| self.sliders.get(slider.index()) == Some(&id))
    }
}

fn column_style(scales: &Scales, display: Display) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let gap = length(scales.spacing.xs as f32);
    Style {
        display,
        flex_direction: FlexDirection::Column,
        flex_shrink: 0.0,
        gap: Size {
            width: gap,
            height: gap,
        },
        ..Default::default()
    }
}

/// Adds the hidden, disabled editors as the last child of `parent`.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `parent` doesn't exist. On any
/// failure the partly built column is removed again.
pub fn insert_adjust_controls(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
) -> Result<AdjustControls, WidgetError> {
    let root = widgets::insert_container(tree, parent, column_style(scales, Display::None))?;
    let built = (|| {
        let mut groups = Vec::new();
        for _ in AdjustKind::ALL {
            groups.push(widgets::insert_container(
                tree,
                root,
                column_style(scales, Display::None),
            )?);
        }
        let mut sliders = Vec::new();
        let mut readouts = Vec::new();
        let mut preserve = None;
        for slider in AdjustSlider::ALL {
            let group = groups
                .get(slider.kind().index())
                .copied()
                .ok_or(WidgetError::UnknownWidget(root))?;
            let (min, max) = slider.range();
            let start = if slider == AdjustSlider::Gamma {
                1.0
            } else if slider == AdjustSlider::Density {
                f64::from(aurora_core::DEFAULT_DENSITY)
            } else {
                0.0
            };
            readouts.push(widgets::insert_label(
                tree,
                group,
                scales,
                slider.readout(start),
            )?);
            sliders.push(widgets::insert_slider(
                tree,
                group,
                scales,
                slider.label(),
                start,
                min,
                max,
            )?);
            if slider == AdjustSlider::Blue {
                preserve = Some(widgets::insert_checkbox(
                    tree,
                    group,
                    scales,
                    "Preserve Luminosity",
                )?);
            }
        }
        let (Some(preserve), Ok(sliders), Ok(readouts), Ok(groups)) = (
            preserve,
            <[WidgetId; 9]>::try_from(sliders),
            <[WidgetId; 9]>::try_from(readouts),
            <[WidgetId; 3]>::try_from(groups),
        ) else {
            return Err(WidgetError::UnknownWidget(root));
        };
        let controls = AdjustControls {
            root,
            groups,
            sliders,
            readouts,
            preserve,
        };
        set_shown(tree, controls, None)?;
        Ok(controls)
    })();
    if built.is_err() {
        let _ = tree.remove(root);
    }
    built
}

fn set_display(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    display: Display,
) -> Result<bool, WidgetError> {
    let mut style = tree.style(id).cloned().ok_or(WidgetError::UnknownWidget(id))?;
    if style.display == display {
        return Ok(false);
    }
    style.display = display;
    tree.set_style(id, style)?;
    Ok(true)
}

/// Shows exactly the group of `kind` (none: the whole column hides) and
/// enables only its widgets.
fn set_shown(
    tree: &mut WidgetTree<WidgetKind>,
    controls: AdjustControls,
    kind: Option<AdjustKind>,
) -> Result<bool, WidgetError> {
    let mut changed = set_display(
        tree,
        controls.root,
        if kind.is_some() {
            Display::Flex
        } else {
            Display::None
        },
    )?;
    for each in AdjustKind::ALL {
        let live = kind == Some(each);
        if let Some(&group) = controls.groups.get(each.index()) {
            changed |= set_display(tree, group, if live { Display::Flex } else { Display::None })?;
        }
    }
    for slider in AdjustSlider::ALL {
        let (Some(&id), Some(&readout)) = (
            controls.sliders.get(slider.index()),
            controls.readouts.get(slider.index()),
        ) else {
            continue;
        };
        let live = kind == Some(slider.kind());
        let disabled = match tree.payload(id) {
            Some(WidgetKind::Slider(state)) => state.disabled,
            Some(_) => return Err(WidgetError::WrongWidgetKind(id)),
            None => return Err(WidgetError::UnknownWidget(id)),
        };
        if disabled == live {
            widgets::set_slider_disabled(tree, id, !live)?;
            changed = true;
        }
        changed |= widgets::set_label_disabled(tree, readout, !live)?;
    }
    let checkbox_disabled = match tree.payload(controls.preserve) {
        Some(WidgetKind::Checkbox(state)) => state.disabled,
        Some(_) => return Err(WidgetError::WrongWidgetKind(controls.preserve)),
        None => return Err(WidgetError::UnknownWidget(controls.preserve)),
    };
    let checkbox_live = kind == Some(AdjustKind::PhotoFilter);
    if checkbox_disabled == checkbox_live {
        widgets::set_checkbox_disabled(tree, controls.preserve, !checkbox_live)?;
        changed = true;
    }
    Ok(changed)
}

/// Which editor group is shown (the active layer's kind as of the last
/// [`sync_adjust_controls`]), `None` when the column is hidden.
#[must_use]
pub fn adjust_controls_shown(
    tree: &WidgetTree<WidgetKind>,
    controls: AdjustControls,
) -> Option<AdjustKind> {
    if tree
        .style(controls.root)
        .is_none_or(|style| style.display == Display::None)
    {
        return None;
    }
    AdjustKind::ALL.into_iter().find(|kind| {
        controls
            .groups
            .get(kind.index())
            .and_then(|&group| tree.style(group))
            .is_some_and(|style| style.display != Display::None)
    })
}

/// Whether Photo Filter's Preserve Luminosity checkbox is checked.
#[must_use]
pub fn adjust_preserve_checked(tree: &WidgetTree<WidgetKind>, controls: AdjustControls) -> bool {
    matches!(
        tree.payload(controls.preserve),
        Some(WidgetKind::Checkbox(state)) if state.checked == accesskit::Toggled::True
    )
}

/// Mirrors `active` into `controls`, in place — never inserting or
/// removing a widget. Returns whether anything changed (cheap enough to
/// run every event-loop iteration).
///
/// - `active` not one of the three adjustments (or `None`): hidden and
///   disabled.
/// - Otherwise that kind's group is shown; every slider of it shows the
///   document's value (a slider equal to `captured` is left alone:
///   mid-drag it is the source of truth) and every readout follows.
///
/// # Errors
///
/// Whatever the underlying widget setters return for an id that isn't the
/// widget kind `controls` says it is.
pub fn sync_adjust_controls(
    tree: &mut WidgetTree<WidgetKind>,
    controls: AdjustControls,
    layers: &LayerTree,
    active: Option<LayerId>,
    captured: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    let Some(adjustment) = active.and_then(|id| adjust_of(layers, id)) else {
        return set_shown(tree, controls, None);
    };
    let kind = AdjustKind::of(adjustment);
    let mut changed = set_shown(tree, controls, kind)?;
    if let Some(filter) = adjustment.photo_filter() {
        let checked = if filter.preserve_luminosity() {
            accesskit::Toggled::True
        } else {
            accesskit::Toggled::False
        };
        if adjust_preserve_checked(tree, controls) != filter.preserve_luminosity() {
            widgets::set_checkbox_checked(tree, controls.preserve, checked)?;
            changed = true;
        }
    }
    for slider in AdjustSlider::ALL.into_iter().filter(|s| Some(s.kind()) == kind) {
        let (Some(&id), Some(&readout)) = (
            controls.sliders.get(slider.index()),
            controls.readouts.get(slider.index()),
        ) else {
            continue;
        };
        let Some(value) = slider.value_of(adjustment) else {
            continue;
        };
        if captured != Some(id) {
            let shown = match tree.payload(id) {
                Some(WidgetKind::Slider(state)) => state.value,
                _ => return Err(WidgetError::WrongWidgetKind(id)),
            };
            if shown.to_bits() != value.to_bits() {
                widgets::set_slider_value(tree, id, value)?;
                changed = true;
            }
        }
        let shown_value = match tree.payload(id) {
            Some(WidgetKind::Slider(state)) => slider.snap(state.value),
            _ => value,
        };
        changed |= widgets::set_label_text(tree, readout, &slider.readout(shown_value))?;
    }
    Ok(changed)
}

/// Whether `id` is (or lies inside) the editors' column.
#[must_use]
pub fn adjust_controls_contains(
    tree: &WidgetTree<WidgetKind>,
    controls: &AdjustControls,
    id: WidgetId,
) -> bool {
    tree.is_within(controls.root, id)
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::indexing_slicing)]
mod tests {
    use super::{
        AdjustKind, AdjustSlider as S, adjust_controls_shown, adjust_preserve_checked,
        insert_adjust_controls, sync_adjust_controls,
    };
    use aurora_core::{ExposureParams, PhotoFilterParams, VibranceParams};
    use aurora_doc::{Adjustment, LayerTree};
    use aurora_widgets::widgets::{self, WidgetKind};

    fn scales() -> aurora_theme::Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn slider_state(
        tree: &aurora_widgets::WidgetTree<WidgetKind>,
        id: aurora_widgets::WidgetId,
    ) -> (f64, bool) {
        match tree.payload(id) {
            Some(WidgetKind::Slider(state)) => (state.value, state.disabled),
            other => unreachable!("{other:?}"),
        }
    }

    fn vib(v: f32, s: f32) -> Adjustment {
        match VibranceParams::new(v, s) {
            Ok(p) => Adjustment::Vibrance(p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn exp(e: f32, o: f32, g: f32) -> Adjustment {
        match ExposureParams::new(e, o, g) {
            Ok(p) => Adjustment::Exposure(p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn filter(colour: [f32; 3], d: f32, p: bool) -> Adjustment {
        match PhotoFilterParams::new(colour, d, p) {
            Ok(p) => Adjustment::PhotoFilter(p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn the_editor_shows_the_group_of_the_active_layer_kind() {
        let scales = scales();
        let mut ws = crate::build_workspace(&scales);
        let controls = match insert_adjust_controls(&mut ws.tree, ws.properties.root, &scales) {
            Ok(c) => c,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(adjust_controls_shown(&ws.tree, controls), None);
        let mut layers = LayerTree::new();
        fn add(layers: &mut LayerTree, name: &str, adjustment: Adjustment) -> aurora_doc::LayerId {
            let Ok(id) = layers.add_adjustment_layer_at(name, adjustment, None, 0) else {
                unreachable!("adjustment layer");
            };
            id
        }
        let v = add(&mut layers, "V", vib(35.0, -12.0));
        let e = add(&mut layers, "E", exp(-1.25, 0.125, 2.2));
        let f = add(&mut layers, "F", filter([0.0, 0.5, 1.0], 61.0, false));
        let hs = add(
            &mut layers,
            "H",
            Adjustment::HueSaturation(aurora_core::HueSaturationParams::identity()),
        );
        fn show(
            ws: &mut crate::Workspace,
            controls: super::AdjustControls,
            layers: &LayerTree,
            id: Option<aurora_doc::LayerId>,
        ) -> Result<bool, aurora_widgets::WidgetError> {
            sync_adjust_controls(&mut ws.tree, controls, layers, id, None)
        }
        assert_eq!(show(&mut ws, controls, &layers, None).ok(), Some(false));
        assert_eq!(show(&mut ws, controls, &layers, Some(v)).ok(), Some(true));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::Vibrance)
        );
        let at = |ws: &crate::Workspace, s: S| slider_state(&ws.tree, controls.sliders[s.index()]);
        assert_eq!(at(&ws, S::Vibrance), (35.0, false));
        assert_eq!(at(&ws, S::VibranceSaturation), (-12.0, false));
        assert!(at(&ws, S::Exposure).1 && at(&ws, S::Red).1, "other groups disabled");
        match ws.tree.payload(controls.readouts[S::VibranceSaturation.index()]) {
            Some(WidgetKind::Label(label)) => assert_eq!(label.text, "Saturation: -12"),
            other => unreachable!("{other:?}"),
        }
        let _ = show(&mut ws, controls, &layers, Some(e));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::Exposure)
        );
        assert_eq!(at(&ws, S::Exposure), (-1.25, false));
        assert_eq!(at(&ws, S::Offset), (0.125, false));
        assert!((at(&ws, S::Gamma).0 - 2.2).abs() < 1e-6);
        assert!(at(&ws, S::Vibrance).1);
        match ws.tree.payload(controls.readouts[S::Exposure.index()]) {
            Some(WidgetKind::Label(label)) => assert_eq!(label.text, "Exposure: -1.25"),
            other => unreachable!("{other:?}"),
        }
        match ws.tree.payload(controls.readouts[S::Offset.index()]) {
            Some(WidgetKind::Label(label)) => assert_eq!(label.text, "Offset: 0.125"),
            other => unreachable!("{other:?}"),
        }
        let _ = show(&mut ws, controls, &layers, Some(f));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::PhotoFilter)
        );
        assert_eq!(at(&ws, S::Density), (61.0, false));
        assert_eq!(at(&ws, S::Red), (0.0, false));
        assert_eq!(at(&ws, S::Green), (128.0, false));
        assert_eq!(at(&ws, S::Blue), (255.0, false));
        assert!(!adjust_preserve_checked(&ws.tree, controls));
        // The checkbox follows the document.
        let _ = layers.set_adjustment(f, filter([0.0, 0.5, 1.0], 61.0, true));
        let _ = show(&mut ws, controls, &layers, Some(f));
        assert!(adjust_preserve_checked(&ws.tree, controls));
        // A captured slider keeps its mid-drag value; released, the document wins.
        let density = controls.sliders[S::Density.index()];
        let _ = widgets::set_slider_value(&mut ws.tree, density, 80.0);
        let _ = sync_adjust_controls(&mut ws.tree, controls, &layers, Some(f), Some(density));
        assert_eq!(slider_state(&ws.tree, density).0, 80.0);
        let _ = show(&mut ws, controls, &layers, Some(f));
        assert_eq!(slider_state(&ws.tree, density).0, 61.0);
        // Another adjustment kind hides it all.
        assert_eq!(show(&mut ws, controls, &layers, Some(hs)).ok(), Some(true));
        assert_eq!(adjust_controls_shown(&ws.tree, controls), None);
        assert!(at(&ws, S::Density).1);
    }

    #[test]
    fn slider_values_snap_to_their_decimals_and_clamp() {
        let v = vib(0.0, 0.0);
        let e = exp(0.0, 0.0, 1.0);
        let f = filter([1.0, 1.0, 1.0], 25.0, true);
        for s in S::ALL {
            let base = match s.kind() {
                AdjustKind::Vibrance => &v,
                AdjustKind::Exposure => &e,
                AdjustKind::PhotoFilter => &f,
            };
            let (min, max) = s.range();
            let lo = s.with_value(base, min - 50.0).and_then(|a| s.value_of(&a));
            let hi = s.with_value(base, max + 50.0).and_then(|a| s.value_of(&a));
            assert!((lo.unwrap_or(f64::NAN) - min).abs() < 1e-6, "{s:?} {lo:?}");
            assert!((hi.unwrap_or(f64::NAN) - max).abs() < 1e-6, "{s:?} {hi:?}");
            assert!(s.with_value(base, f64::NAN).is_none());
            assert!(s.with_value(base, f64::INFINITY).is_none());
        }
        // The wrong kind of adjustment gives nothing.
        assert!(S::Vibrance.with_value(&e, 5.0).is_none());
        assert!(S::Gamma.with_value(&f, 2.0).is_none());
        assert!(S::Red.with_value(&v, 2.0).is_none());
        // Rounding.
        let got = |s: S, base: &Adjustment, x: f64| s.with_value(base, x).and_then(|a| s.value_of(&a));
        assert_eq!(got(S::Vibrance, &v, 40.6), Some(41.0));
        assert!((got(S::Exposure, &e, 1.2349).unwrap_or(0.0) - 1.23).abs() < 1e-6);
        assert!((got(S::Offset, &e, 0.12349).unwrap_or(0.0) - 0.123).abs() < 1e-6);
        assert!((got(S::Gamma, &e, 2.226).unwrap_or(0.0) - 2.23).abs() < 1e-6);
        assert_eq!(got(S::Density, &f, 33.4), Some(33.0));
        assert_eq!(got(S::Green, &f, 138.4), Some(138.0));
    }

    #[test]
    fn a_slider_edit_changes_only_its_own_field() {
        let e = exp(1.0, 0.25, 2.0);
        let next = S::Offset.with_value(&e, -0.25);
        let p = next.as_ref().and_then(Adjustment::exposure);
        assert_eq!(
            p.map(|p| (p.exposure(), p.offset(), p.gamma())),
            Some((1.0, -0.25, 2.0))
        );
        let f = filter([0.2, 0.4, 0.6], 30.0, false);
        let next = S::Blue.with_value(&f, 255.0);
        let p = next.as_ref().and_then(Adjustment::photo_filter);
        assert_eq!(
            p.map(|p| (p.colour()[0], p.colour()[1], p.colour()[2], p.density())),
            Some((0.2, 0.4, 1.0, 30.0))
        );
        assert_eq!(p.map(|p| p.preserve_luminosity()), Some(false));
        let v = vib(10.0, 20.0);
        let next = S::VibranceSaturation.with_value(&v, -30.0);
        let p = next.as_ref().and_then(Adjustment::vibrance);
        assert_eq!(p.map(|p| (p.vibrance(), p.saturation())), Some((10.0, -30.0)));
    }

    #[test]
    fn the_preset_colours_survive_the_8_bit_sliders_exactly() {
        for (_, [r, g, b]) in aurora_core::PHOTO_FILTER_PRESETS {
            let colour = [r, g, b].map(|c| f32::from(c) / 255.0);
            let f = filter(colour, 25.0, true);
            let seen = [S::Red, S::Green, S::Blue].map(|s| s.value_of(&f));
            assert_eq!(seen, [r, g, b].map(|c| Some(f64::from(c))));
            // And re-applying what the slider shows changes nothing.
            let again = S::Green.with_value(&f, seen[1].unwrap_or(0.0));
            assert_eq!(again, Some(f));
        }
    }
}
