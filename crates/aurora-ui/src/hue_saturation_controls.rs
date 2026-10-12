//! The Properties panel's **Hue/Saturation editor** (0.203.0): three
//! master sliders (Hue, Saturation, Lightness), a "Colorize" checkbox and
//! the three Colorize sliders, each slider with a readout label, shown
//! only while the active layer is a Hue/Saturation adjustment.
//!
//! **Where it lives.** Inside the Properties panel's tool-controls strip
//! ([`crate::insert_tool_controls`] builds it beside the Levels column),
//! so every pointer, key and accessibility event on it reaches
//! `aurora-app` through the same `ToolControls` routing the Levels and
//! Curves editors use. It starts hidden (`Display::None`) and disabled.
//!
//! **This module knows nothing about undo or pixels**: it builds the
//! widgets, maps a slider to a field of a [`HueSaturationParams`]
//! ([`HueSaturationSlider::with_value`]) and mirrors a document into the
//! widgets ([`sync_hue_saturation_controls`]). Turning a change into a
//! document edit (one undo step per drag, one per Colorize toggle) is
//! `aurora-app`'s job.
//!
//! **Which sliders are live.** With Colorize off the three Colorize
//! sliders (and their readouts) are disabled; with it on the three master
//! sliders are, because the adjustment ignores them then. The values of
//! the disabled set stay shown, so toggling back changes nothing else.
//! (Photoshop reuses one set of sliders for both modes; two sets keeps
//! each mode's values apart, as the document stores them.)
//!
//! **Range selector (0.204.0).** A tab bar above the first three
//! sliders picks what they edit: Master (tab 0) or one of the six colour
//! ranges (tabs 1..=6, reds first). The selection is widget state only —
//! never document state, no undo step. The range edges are not editable
//! here; they import from a PSD and default to Photoshop's standard ones.
//! The Colorize sliders ignore the selection; with Colorize on the tab bar
//! is disabled (Colorize has no ranges).
//!
//! **Units.** Whole numbers throughout (Photoshop's fields): hue in
//! degrees, saturation and lightness in percent.

use aurora_core::{
    HUE_RANGE_COUNT, HUE_RANGE_NAMES, HueRange, HueSaturationParams, HueSaturationRangesParams,
};
use aurora_doc::{Adjustment, LayerId, LayerTree};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{Display, FlexDirection, Size, Style};

/// One of the six sliders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HueSaturationSlider {
    /// The master hue shift, `-180..=180` degrees.
    Hue,
    /// The master saturation, `-100..=100`.
    Saturation,
    /// The master lightness, `-100..=100`.
    Lightness,
    /// The Colorize hue, `0..=360` degrees.
    ColorizeHue,
    /// The Colorize saturation, `0..=100`.
    ColorizeSaturation,
    /// The Colorize lightness, `-100..=100`.
    ColorizeLightness,
}

impl HueSaturationSlider {
    /// Every slider, in layout order — the one place that order is fixed.
    pub const ALL: [Self; 6] = [
        Self::Hue,
        Self::Saturation,
        Self::Lightness,
        Self::ColorizeHue,
        Self::ColorizeSaturation,
        Self::ColorizeLightness,
    ];

    /// The slider's name (its accessible label and its readout's prefix).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Lightness => "Lightness",
            Self::ColorizeHue => "Colorize hue",
            Self::ColorizeSaturation => "Colorize saturation",
            Self::ColorizeLightness => "Colorize lightness",
        }
    }

    /// Position in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Hue => 0,
            Self::Saturation => 1,
            Self::Lightness => 2,
            Self::ColorizeHue => 3,
            Self::ColorizeSaturation => 4,
            Self::ColorizeLightness => 5,
        }
    }

    /// Whether this is one of the three Colorize sliders.
    #[must_use]
    pub const fn is_colorize(self) -> bool {
        matches!(
            self,
            Self::ColorizeHue | Self::ColorizeSaturation | Self::ColorizeLightness
        )
    }

    /// The slider's `(min, max)`.
    #[must_use]
    pub const fn range(self) -> (f64, f64) {
        match self {
            Self::Hue => (-180.0, 180.0),
            Self::ColorizeHue => (0.0, 360.0),
            Self::ColorizeSaturation => (0.0, 100.0),
            Self::Saturation | Self::Lightness | Self::ColorizeLightness => (-100.0, 100.0),
        }
    }

    /// The slider position for `params`; the first three sliders read
    /// `range` (0 the master, `1..=6` a colour range, a larger index the
    /// master).
    #[must_use]
    pub fn value_of(self, params: &HueSaturationRangesParams, range: usize) -> f64 {
        let base = params.base();
        let ranged = range.checked_sub(1).and_then(|i| params.range(i));
        f64::from(match (self, ranged) {
            (Self::Hue, Some(r)) => r.hue(),
            (Self::Saturation, Some(r)) => r.saturation(),
            (Self::Lightness, Some(r)) => r.lightness(),
            (Self::Hue, None) => base.hue(),
            (Self::Saturation, None) => base.saturation(),
            (Self::Lightness, None) => base.lightness(),
            (Self::ColorizeHue, _) => base.colorize_hue(),
            (Self::ColorizeSaturation, _) => base.colorize_saturation(),
            (Self::ColorizeLightness, _) => base.colorize_lightness(),
        })
    }

    /// The readout text for `value`.
    #[must_use]
    pub fn readout(self, value: f64) -> String {
        format!("{}: {value:.0}", self.label())
    }

    /// `params` with this slider moved to `value` (rounded to a whole
    /// number and clamped to the slider's range), on `range` for the
    /// first three sliders. `None` for a non-finite `value`.
    #[must_use]
    pub fn with_value(
        self,
        params: &HueSaturationRangesParams,
        range: usize,
        value: f64,
    ) -> Option<HueSaturationRangesParams> {
        if !value.is_finite() {
            return None;
        }
        let (min, max) = self.range();
        #[allow(clippy::cast_possible_truncation)]
        let v = value.round().clamp(min, max) as f32;
        let base = params.base();
        if let (Some(index), false) = (range.checked_sub(1), self.is_colorize()) {
            let current: &HueRange = params.range(index)?;
            let next = match self {
                Self::Hue => current.with_hue(v),
                Self::Saturation => current.with_saturation(v),
                _ => current.with_lightness(v),
            }
            .ok()?;
            return params.with_range(index, next);
        }
        let next = match self {
            Self::Hue => base.with_hue(v),
            Self::Saturation => base.with_saturation(v),
            Self::Lightness => base.with_lightness(v),
            Self::ColorizeHue => base.with_colorize_hue(v),
            Self::ColorizeSaturation => base.with_colorize_saturation(v),
            Self::ColorizeLightness => base.with_colorize_lightness(v),
        }
        .ok()?;
        Some(params.with_base(next))
    }
}

/// `id`'s Hue/Saturation settings (master/Colorize and the six colour
/// ranges, whichever variant stores them), `None` for anything that is not
/// a Hue/Saturation adjustment layer.
#[must_use]
pub fn hue_saturation_params(layers: &LayerTree, id: LayerId) -> Option<HueSaturationRangesParams> {
    layers.adjustment(id).and_then(Adjustment::hue_saturation)
}

/// The tab labels of the range selector: Master, then the six ranges.
const RANGE_TAB_LABELS: [&str; HUE_RANGE_COUNT + 1] =
    ["Master", "Red", "Yel", "Grn", "Cyn", "Blu", "Mag"];

/// The accessible names of the range tabs.
fn range_tab_names() -> Vec<String> {
    std::iter::once("Master")
        .chain(HUE_RANGE_NAMES)
        .map(str::to_owned)
        .collect()
}

/// The six sliders, their readouts, the Colorize checkbox and the column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HueSaturationControls {
    /// The column container.
    pub root: WidgetId,
    /// The range selector tab bar: tab 0 Master, tabs 1..=6 the ranges.
    pub range: WidgetId,
    /// The "Colorize" checkbox.
    pub colorize: WidgetId,
    /// The sliders, indexed by [`HueSaturationSlider::index`].
    pub sliders: [WidgetId; 6],
    /// Their readout labels, same order.
    pub readouts: [WidgetId; 6],
}

impl HueSaturationControls {
    /// Which slider `id` is, if any.
    #[must_use]
    pub fn slider_of(&self, id: WidgetId) -> Option<HueSaturationSlider> {
        HueSaturationSlider::ALL
            .into_iter()
            .find(|slider| self.sliders.get(slider.index()) == Some(&id))
    }
}

fn column_style(scales: &Scales) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let gap = length(scales.spacing.xs as f32);
    Style {
        display: Display::None,
        flex_direction: FlexDirection::Column,
        flex_shrink: 0.0,
        gap: Size {
            width: gap,
            height: gap,
        },
        ..Default::default()
    }
}

/// Adds the hidden, disabled Hue/Saturation editor as the last child of
/// `parent`: master readouts and sliders, the checkbox, then the Colorize
/// readouts and sliders.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `parent` doesn't exist. On any
/// failure the partly built column is removed again.
pub fn insert_hue_saturation_controls(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
) -> Result<HueSaturationControls, WidgetError> {
    let root = widgets::insert_container(tree, parent, column_style(scales))?;
    let built = (|| {
        let identity = HueSaturationRangesParams::plain(HueSaturationParams::identity());
        let range = widgets::insert_tab_bar(
            tree,
            root,
            scales,
            "Hue/Saturation range",
            RANGE_TAB_LABELS.iter().map(|&l| l.to_owned()).collect(),
            0,
        )?;
        widgets::set_tab_accessible_names(tree, range, range_tab_names())?;
        let mut sliders = Vec::new();
        let mut readouts = Vec::new();
        let mut colorize = None;
        for slider in HueSaturationSlider::ALL {
            if slider == HueSaturationSlider::ColorizeHue {
                colorize = Some(widgets::insert_checkbox(tree, root, scales, "Colorize")?);
            }
            let value = slider.value_of(&identity, 0);
            let (min, max) = slider.range();
            readouts.push(widgets::insert_label(
                tree,
                root,
                scales,
                slider.readout(value),
            )?);
            sliders.push(widgets::insert_slider(
                tree,
                root,
                scales,
                slider.label(),
                value,
                min,
                max,
            )?);
        }
        let (Some(colorize), Ok(sliders), Ok(readouts)) = (
            colorize,
            <[WidgetId; 6]>::try_from(sliders),
            <[WidgetId; 6]>::try_from(readouts),
        ) else {
            return Err(WidgetError::UnknownWidget(root));
        };
        let controls = HueSaturationControls {
            root,
            range,
            colorize,
            sliders,
            readouts,
        };
        set_enabled(tree, controls, false, false)?;
        Ok(controls)
    })();
    if built.is_err() {
        let _ = tree.remove(root);
    }
    built
}

/// Enables the checkbox and exactly one slider set: `shown` false disables
/// everything; otherwise the Colorize sliders are live iff `colorize`, the
/// master ones iff not.
fn set_enabled(
    tree: &mut WidgetTree<WidgetKind>,
    controls: HueSaturationControls,
    shown: bool,
    colorize: bool,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    let tabs_disabled = widgets::tab_bar_state(tree, controls.range)?.is_disabled();
    if tabs_disabled == (shown && !colorize) {
        widgets::set_tab_bar_disabled(tree, controls.range, !shown || colorize)?;
        changed = true;
    }
    let checkbox_disabled = match tree.payload(controls.colorize) {
        Some(WidgetKind::Checkbox(state)) => state.disabled,
        Some(_) => return Err(WidgetError::WrongWidgetKind(controls.colorize)),
        None => return Err(WidgetError::UnknownWidget(controls.colorize)),
    };
    if checkbox_disabled == shown {
        widgets::set_checkbox_disabled(tree, controls.colorize, !shown)?;
        changed = true;
    }
    for slider in HueSaturationSlider::ALL {
        let (Some(&id), Some(&readout)) = (
            controls.sliders.get(slider.index()),
            controls.readouts.get(slider.index()),
        ) else {
            continue;
        };
        let live = shown && slider.is_colorize() == colorize;
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
    Ok(changed)
}

fn set_shown(
    tree: &mut WidgetTree<WidgetKind>,
    controls: HueSaturationControls,
    shown: bool,
    colorize: bool,
) -> Result<bool, WidgetError> {
    let display = if shown { Display::Flex } else { Display::None };
    let mut style = tree
        .style(controls.root)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(controls.root))?;
    let mut changed = false;
    if style.display != display {
        style.display = display;
        tree.set_style(controls.root, style)?;
        changed = true;
    }
    Ok(set_enabled(tree, controls, shown, colorize)? || changed)
}

/// Whether the Hue/Saturation editor is shown (the active layer is a
/// Hue/Saturation adjustment, as of the last
/// [`sync_hue_saturation_controls`]).
#[must_use]
pub fn hue_saturation_controls_shown(
    tree: &WidgetTree<WidgetKind>,
    controls: HueSaturationControls,
) -> bool {
    tree.style(controls.root)
        .is_some_and(|style| style.display != Display::None)
}

/// The range the first three sliders edit: 0 the master, `1..=6` a
/// colour range (the selector's selected tab; 0 if it cannot be read).
#[must_use]
pub fn hue_saturation_selected_range(
    tree: &WidgetTree<WidgetKind>,
    controls: HueSaturationControls,
) -> usize {
    widgets::tab_bar_state(tree, controls.range)
        .map_or(0, aurora_widgets::widgets::TabBarState::selected)
}

/// Whether the Colorize checkbox is checked.
#[must_use]
pub fn hue_saturation_colorize_checked(
    tree: &WidgetTree<WidgetKind>,
    controls: HueSaturationControls,
) -> bool {
    matches!(
        tree.payload(controls.colorize),
        Some(WidgetKind::Checkbox(state)) if state.checked == accesskit::Toggled::True
    )
}

/// Mirrors `active` into `controls`, in place — never inserting or
/// removing a widget. Returns whether anything changed (cheap enough to
/// run every event-loop iteration).
///
/// - `active` not a Hue/Saturation layer (or `None`): hidden and disabled.
/// - Otherwise shown; the checkbox and every slider show the document's
///   value (a slider equal to `captured` is left alone: mid-drag it is the
///   source of truth), every readout follows, and the live slider set
///   follows the Colorize flag.
///
/// # Errors
///
/// Whatever the underlying widget setters return for an id that isn't the
/// widget kind `controls` says it is.
pub fn sync_hue_saturation_controls(
    tree: &mut WidgetTree<WidgetKind>,
    controls: HueSaturationControls,
    layers: &LayerTree,
    active: Option<LayerId>,
    captured: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    let Some(params) = active.and_then(|id| hue_saturation_params(layers, id)) else {
        return set_shown(tree, controls, false, false);
    };
    let range = hue_saturation_selected_range(tree, controls);
    let params = &params;
    let colorize_on = params.base().colorize();
    let mut changed = set_shown(tree, controls, true, colorize_on)?;
    let checked = if colorize_on {
        accesskit::Toggled::True
    } else {
        accesskit::Toggled::False
    };
    if hue_saturation_colorize_checked(tree, controls) != colorize_on {
        widgets::set_checkbox_checked(tree, controls.colorize, checked)?;
        changed = true;
    }
    for slider in HueSaturationSlider::ALL {
        let (Some(&id), Some(&readout)) = (
            controls.sliders.get(slider.index()),
            controls.readouts.get(slider.index()),
        ) else {
            continue;
        };
        let value = slider.value_of(params, range);
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
            Some(WidgetKind::Slider(state)) => state.value.round(),
            _ => value,
        };
        changed |= widgets::set_label_text(tree, readout, &slider.readout(shown_value))?;
    }
    Ok(changed)
}

/// Whether `id` is (or lies inside) the Hue/Saturation editor's column.
#[must_use]
pub fn hue_saturation_controls_contains(
    tree: &WidgetTree<WidgetKind>,
    controls: &HueSaturationControls,
    id: WidgetId,
) -> bool {
    tree.is_within(controls.root, id)
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::indexing_slicing)]
mod tests {
    use super::{
        HueSaturationSlider as S, hue_saturation_colorize_checked, hue_saturation_controls_shown,
        insert_hue_saturation_controls, sync_hue_saturation_controls,
    };
    use aurora_core::{HueSaturationParams, HueSaturationRangesParams};
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

    fn st(ws: &crate::Workspace, c: super::HueSaturationControls, i: usize) -> (f64, bool) {
        slider_state(&ws.tree, c.sliders[i])
    }

    fn params(h: f32, s: f32, l: f32) -> HueSaturationParams {
        match HueSaturationParams::new(h, s, l, false, 0.0, 25.0, 0.0) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn the_editor_follows_the_active_layer_and_the_colorize_flag() {
        let scales = scales();
        let mut ws = crate::build_workspace(&scales);
        let controls =
            match insert_hue_saturation_controls(&mut ws.tree, ws.properties.root, &scales) {
                Ok(c) => c,
                Err(err) => unreachable!("{err:?}"),
            };
        assert!(!hue_saturation_controls_shown(&ws.tree, controls));
        let mut layers = LayerTree::new();
        let Ok(id) = layers.add_adjustment_layer_at(
            "Hue/Saturation 1",
            Adjustment::HueSaturation(params(-27.0, 12.0, -64.0)),
            None,
            0,
        ) else {
            unreachable!("adjustment layer");
        };
        assert_eq!(
            sync_hue_saturation_controls(&mut ws.tree, controls, &layers, None, None).ok(),
            Some(false)
        );
        assert_eq!(
            sync_hue_saturation_controls(&mut ws.tree, controls, &layers, Some(id), None).ok(),
            Some(true)
        );
        assert!(hue_saturation_controls_shown(&ws.tree, controls));
        assert!(!hue_saturation_colorize_checked(&ws.tree, controls));
        assert_eq!(st(&ws, controls, S::Hue.index()), (-27.0, false));
        assert_eq!(st(&ws, controls, S::Saturation.index()), (12.0, false));
        assert_eq!(st(&ws, controls, S::Lightness.index()), (-64.0, false));
        // Colorize off: its sliders are disabled but show the stored values.
        assert_eq!(
            st(&ws, controls, S::ColorizeSaturation.index()),
            (25.0, true)
        );
        match ws.tree.payload(controls.readouts[S::Hue.index()]) {
            Some(WidgetKind::Label(label)) => assert_eq!(label.text, "Hue: -27"),
            other => unreachable!("{other:?}"),
        }
        // Colorize on: the sets swap.
        let on = params(-27.0, 12.0, -64.0).with_colorize(true);
        let _ = layers.set_adjustment(id, Adjustment::HueSaturation(on));
        let _ = sync_hue_saturation_controls(&mut ws.tree, controls, &layers, Some(id), None);
        assert!(hue_saturation_colorize_checked(&ws.tree, controls));
        assert!(
            widgets::tab_bar_state(&ws.tree, controls.range)
                .is_ok_and(aurora_widgets::widgets::TabBarState::is_disabled),
            "Colorize has no ranges: the selector is disabled"
        );
        assert!(
            st(&ws, controls, S::Hue.index()).1 && !st(&ws, controls, S::ColorizeHue.index()).1
        );
        // A captured slider keeps its mid-drag value.
        let hue = controls.sliders[S::ColorizeHue.index()];
        let _ = widgets::set_slider_value(&mut ws.tree, hue, 200.0);
        let _ = sync_hue_saturation_controls(&mut ws.tree, controls, &layers, Some(id), Some(hue));
        assert_eq!(slider_state(&ws.tree, hue).0, 200.0);
        let _ = sync_hue_saturation_controls(&mut ws.tree, controls, &layers, Some(id), None);
        assert_eq!(
            slider_state(&ws.tree, hue).0,
            0.0,
            "released: the document wins"
        );
        assert_eq!(
            sync_hue_saturation_controls(&mut ws.tree, controls, &layers, None, None).ok(),
            Some(true)
        );
        assert!(!hue_saturation_controls_shown(&ws.tree, controls));
        assert!(st(&ws, controls, S::Hue.index()).1 && st(&ws, controls, S::ColorizeHue.index()).1);
    }

    #[test]
    fn slider_values_round_to_whole_numbers_and_clamp() {
        let id = HueSaturationRangesParams::plain(HueSaturationParams::identity());
        for range in 0..=6 {
            for s in S::ALL {
                let (min, max) = s.range();
                let lo = s
                    .with_value(&id, range, min - 50.0)
                    .map(|p| s.value_of(&p, range));
                let hi = s
                    .with_value(&id, range, max + 50.0)
                    .map(|p| s.value_of(&p, range));
                assert_eq!(lo, Some(min), "{s:?} {range}");
                assert_eq!(hi, Some(max), "{s:?} {range}");
                assert!(s.with_value(&id, range, f64::NAN).is_none());
            }
        }
        let moved = S::Saturation.with_value(&id, 0, 40.6);
        assert_eq!(moved.map(|p| p.base().saturation()), Some(41.0));
        let m = moved.unwrap_or(id);
        let b = m.base();
        assert_eq!((b.hue(), b.lightness(), b.colorize()), (0.0, 0.0, false));
        assert_eq!(S::ColorizeSaturation.value_of(&id, 0), 25.0);
        // A range edit leaves the master and the other ranges alone.
        let r = S::Lightness.with_value(&id, 3, -40.0).unwrap_or(id);
        assert_eq!(S::Lightness.value_of(&r, 3), -40.0);
        assert_eq!(S::Lightness.value_of(&r, 0), 0.0);
        assert_eq!(S::Lightness.value_of(&r, 2), 0.0);
        assert_eq!(
            r.range(2).map(aurora_core::HueRange::lightness),
            Some(-40.0)
        );
        assert_eq!(
            r.range(2).map(aurora_core::HueRange::edges),
            Some([75.0, 105.0, 135.0, 165.0])
        );
        // The Colorize sliders ignore the selection.
        let c = S::ColorizeHue.with_value(&id, 4, 99.0).unwrap_or(id);
        assert_eq!(c.base().colorize_hue(), 99.0);
        assert!(c.ranges_adjust_nothing());
        assert!(S::ColorizeHue.is_colorize() && !S::Hue.is_colorize());
    }

    /// 0.204.0: the selector re-binds the first three sliders to a range
    /// of a ranged layer and back to the master, with no document change.
    #[test]
    fn the_range_selector_rebinds_the_first_three_sliders() {
        let scales = scales();
        let mut ws = crate::build_workspace(&scales);
        let controls =
            match insert_hue_saturation_controls(&mut ws.tree, ws.properties.root, &scales) {
                Ok(c) => c,
                Err(err) => unreachable!("{err:?}"),
            };
        let ranged = HueSaturationRangesParams::plain(params(5.0, 6.0, 7.0))
            .with_range(
                2,
                match aurora_core::HueRange::new([75.0, 105.0, 135.0, 165.0], 11.0, -22.0, 33.0) {
                    Ok(r) => r,
                    Err(err) => unreachable!("{err:?}"),
                },
            )
            .unwrap_or_else(|| unreachable!("range"));
        let mut layers = LayerTree::new();
        let Ok(id) = layers.add_adjustment_layer_at(
            "Hue/Saturation 1",
            Adjustment::from_hue_saturation(ranged),
            None,
            0,
        ) else {
            unreachable!("adjustment layer");
        };
        let _ = sync_hue_saturation_controls(&mut ws.tree, controls, &layers, Some(id), None);
        assert_eq!(super::hue_saturation_selected_range(&ws.tree, controls), 0);
        assert_eq!(st(&ws, controls, 0), (5.0, false));
        assert!(widgets::tab_bar_state(&ws.tree, controls.range).is_ok_and(|t| !t.is_disabled()));
        // Select Greens (tab 3).
        let _ = widgets::select_tab(&mut ws.tree, controls.range, 3);
        let _ = sync_hue_saturation_controls(&mut ws.tree, controls, &layers, Some(id), None);
        assert_eq!(super::hue_saturation_selected_range(&ws.tree, controls), 3);
        assert_eq!(st(&ws, controls, 0), (11.0, false));
        assert_eq!(st(&ws, controls, 1), (-22.0, false));
        assert_eq!(st(&ws, controls, 2), (33.0, false));
        match ws.tree.payload(controls.readouts[1]) {
            Some(WidgetKind::Label(label)) => assert_eq!(label.text, "Saturation: -22"),
            other => unreachable!("{other:?}"),
        }
        // Back to Master.
        let _ = widgets::select_tab(&mut ws.tree, controls.range, 0);
        let _ = sync_hue_saturation_controls(&mut ws.tree, controls, &layers, Some(id), None);
        assert_eq!(st(&ws, controls, 0), (5.0, false));
        assert_eq!(
            layers.adjustment(id),
            Some(&Adjustment::from_hue_saturation(ranged))
        );
    }
}
