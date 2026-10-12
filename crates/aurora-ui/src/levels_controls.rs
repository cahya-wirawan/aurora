//! The Properties panel's **Levels editor** (0.202.0): a channel selector
//! (the Curves editor's RGB / Red / Green / Blue tab bar, one per
//! editor) above five sliders — input black, gamma, input white, output
//! black, output white — each with a readout label, shown only while the
//! active layer is a Levels adjustment.
//!
//! **Where it lives.** Inside the Properties panel's tool-controls strip
//! ([`crate::insert_tool_controls`] builds it beside the Curves column),
//! so every pointer, key and accessibility event on it reaches
//! `aurora-app` through the same `ToolControls` routing the radius
//! slider and the Curves editor use. It starts hidden (`Display::None`)
//! and disabled.
//!
//! **This module knows nothing about undo or pixels**: it builds the
//! widgets, maps a slider to a field of a [`LevelsChannel`]
//! ([`LevelsSlider::value_of`], [`LevelsSlider::with_value`]) and mirrors
//! a document into the widgets ([`sync_levels_controls`]). Turning a
//! slider change into a document edit and coalescing a drag into one undo
//! step is `aurora-app`'s job.
//!
//! **Units.** Input and output points are shown as `0..=255` levels and
//! edited in whole levels (Photoshop's 8-bit fields), gamma as
//! `0.10..=9.99` in hundredths; the stored values are `value / 255` and
//! the gamma itself. No histogram is drawn behind the input sliders (the
//! Curves histogram is computed per channel by `aurora-app` and is not a
//! reusable widget yet).

use aurora_core::{GAMMA_MAX, GAMMA_MIN, LevelsChannel, LevelsParams};
use aurora_doc::{Adjustment, LayerId, LayerTree};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, TabBarOutcome, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{Display, FlexDirection, Size, Style};

use crate::curves_controls::CurvesChannel;

/// One of the five sliders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LevelsSlider {
    /// The input black point, `0..=254` levels.
    InputBlack,
    /// The midtone gamma, `0.10..=9.99`.
    Gamma,
    /// The input white point, `1..=255` levels.
    InputWhite,
    /// The output black point, `0..=255` levels.
    OutputBlack,
    /// The output white point, `0..=255` levels.
    OutputWhite,
}

impl LevelsSlider {
    /// Every slider, in layout order — the one place that order is fixed.
    pub const ALL: [Self; 5] = [
        Self::InputBlack,
        Self::Gamma,
        Self::InputWhite,
        Self::OutputBlack,
        Self::OutputWhite,
    ];

    /// The slider's name (its accessible label and its readout's prefix).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::InputBlack => "Input black",
            Self::Gamma => "Gamma",
            Self::InputWhite => "Input white",
            Self::OutputBlack => "Output black",
            Self::OutputWhite => "Output white",
        }
    }

    /// Position in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::InputBlack => 0,
            Self::Gamma => 1,
            Self::InputWhite => 2,
            Self::OutputBlack => 3,
            Self::OutputWhite => 4,
        }
    }

    /// The slider's `(min, max)`.
    #[must_use]
    pub fn range(self) -> (f64, f64) {
        match self {
            Self::Gamma => (f64::from(GAMMA_MIN), f64::from(GAMMA_MAX)),
            _ => (0.0, 255.0),
        }
    }

    /// The slider position of `channel`: levels `0..=255`, or the gamma.
    #[must_use]
    pub fn value_of(self, channel: &LevelsChannel) -> f64 {
        match self {
            Self::InputBlack => levels_of(channel.input_black()),
            Self::Gamma => f64::from(channel.gamma()),
            Self::InputWhite => levels_of(channel.input_white()),
            Self::OutputBlack => levels_of(channel.output_black()),
            Self::OutputWhite => levels_of(channel.output_white()),
        }
    }

    /// The readout text for `value`.
    #[must_use]
    pub fn readout(self, value: f64) -> String {
        match self {
            Self::Gamma => format!("{}: {value:.2}", self.label()),
            _ => format!("{}: {value:.0}", self.label()),
        }
    }

    /// `channel` with this slider moved to `value` (rounded to whole
    /// levels, or to hundredths for gamma, and clamped). The input black
    /// stops one level below the white and the white one above the black,
    /// so a drag can never produce an invalid record. `None` for a
    /// non-finite `value`.
    #[must_use]
    pub fn with_value(self, channel: &LevelsChannel, value: f64) -> Option<LevelsChannel> {
        if !value.is_finite() {
            return None;
        }
        let level = |v: f64| v.round().clamp(0.0, 255.0);
        let ib = level(f64::from(channel.input_black()) * 255.0);
        let iw = level(f64::from(channel.input_white()) * 255.0);
        let ob = level(f64::from(channel.output_black()) * 255.0);
        let ow = level(f64::from(channel.output_white()) * 255.0);
        let gamma = f64::from(channel.gamma());
        let (mut ib, mut iw, mut ob, mut ow, mut gamma) = (ib, iw, ob, ow, gamma);
        match self {
            Self::InputBlack => ib = level(value).min(iw - 1.0),
            Self::InputWhite => iw = level(value).max(ib + 1.0),
            Self::OutputBlack => ob = level(value),
            Self::OutputWhite => ow = level(value),
            Self::Gamma => {
                gamma = ((value * 100.0).round() / 100.0)
                    .clamp(f64::from(GAMMA_MIN), f64::from(GAMMA_MAX));
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        LevelsChannel::new(
            (ib / 255.0) as f32,
            (iw / 255.0) as f32,
            gamma as f32,
            (ob / 255.0) as f32,
            (ow / 255.0) as f32,
        )
        .ok()
    }
}

fn levels_of(unit: f32) -> f64 {
    (f64::from(unit) * 255.0).round()
}

/// The record the selector's `channel` edits inside `params`.
#[must_use]
pub fn channel_levels(params: &LevelsParams, channel: CurvesChannel) -> LevelsChannel {
    match channel {
        CurvesChannel::Rgb => params.master,
        CurvesChannel::Red => params.red,
        CurvesChannel::Green => params.green,
        CurvesChannel::Blue => params.blue,
    }
}

/// `params` with `channel`'s record replaced, every other one untouched.
#[must_use]
pub fn with_channel_levels(
    params: &LevelsParams,
    channel: CurvesChannel,
    record: LevelsChannel,
) -> LevelsParams {
    let mut next = *params;
    match channel {
        CurvesChannel::Rgb => next.master = record,
        CurvesChannel::Red => next.red = record,
        CurvesChannel::Green => next.green = record,
        CurvesChannel::Blue => next.blue = record,
    }
    next
}

/// `id`'s Levels parameters, `None` for anything that is not a Levels
/// adjustment layer.
#[must_use]
pub fn levels_params(layers: &LayerTree, id: LayerId) -> Option<&LevelsParams> {
    match layers.adjustment(id) {
        Some(Adjustment::Levels(params)) => Some(params),
        _ => None,
    }
}

/// The selector, the five sliders and their readouts, plus the column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelsControls {
    /// The column container.
    pub root: WidgetId,
    /// The channel tab bar; tab `i` is `CurvesChannel::ALL[i]`.
    pub channel: WidgetId,
    /// The sliders, indexed by [`LevelsSlider::index`].
    pub sliders: [WidgetId; 5],
    /// Their readout labels, same order.
    pub readouts: [WidgetId; 5],
}

impl LevelsControls {
    /// Which slider `id` is, if any.
    #[must_use]
    pub fn slider_of(&self, id: WidgetId) -> Option<LevelsSlider> {
        LevelsSlider::ALL
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

/// Adds the hidden, disabled Levels editor as the last child of `parent`.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `parent` doesn't exist. On any
/// failure the partly built column is removed again.
pub fn insert_levels_controls(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
) -> Result<LevelsControls, WidgetError> {
    let root = widgets::insert_container(tree, parent, column_style(scales))?;
    let built = (|| {
        let channel = widgets::insert_tab_bar(
            tree,
            root,
            scales,
            "Levels channel",
            CurvesChannel::ALL
                .iter()
                .map(|channel| channel.label().to_owned())
                .collect(),
            CurvesChannel::Rgb.index(),
        )?;
        let identity = LevelsChannel::identity();
        let mut sliders = Vec::new();
        let mut readouts = Vec::new();
        for slider in LevelsSlider::ALL {
            let value = slider.value_of(&identity);
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
        let (Ok(sliders), Ok(readouts)) = (
            <[WidgetId; 5]>::try_from(sliders),
            <[WidgetId; 5]>::try_from(readouts),
        ) else {
            return Err(WidgetError::UnknownWidget(root));
        };
        let controls = LevelsControls {
            root,
            channel,
            sliders,
            readouts,
        };
        set_enabled(tree, controls, false)?;
        Ok(controls)
    })();
    if built.is_err() {
        let _ = tree.remove(root);
    }
    built
}

fn set_enabled(
    tree: &mut WidgetTree<WidgetKind>,
    controls: LevelsControls,
    enabled: bool,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    if widgets::tab_bar_state(tree, controls.channel)?.is_disabled() == enabled {
        widgets::set_tab_bar_disabled(tree, controls.channel, !enabled)?;
        changed = true;
    }
    for &slider in &controls.sliders {
        let disabled = match tree.payload(slider) {
            Some(WidgetKind::Slider(state)) => state.disabled,
            Some(_) => return Err(WidgetError::WrongWidgetKind(slider)),
            None => return Err(WidgetError::UnknownWidget(slider)),
        };
        if disabled == enabled {
            widgets::set_slider_disabled(tree, slider, !enabled)?;
            changed = true;
        }
    }
    for &readout in &controls.readouts {
        changed |= widgets::set_label_disabled(tree, readout, !enabled)?;
    }
    Ok(changed)
}

fn set_shown(
    tree: &mut WidgetTree<WidgetKind>,
    controls: LevelsControls,
    shown: bool,
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
    Ok(set_enabled(tree, controls, shown)? || changed)
}

/// Whether the Levels editor is shown (the active layer is a Levels
/// adjustment, as of the last [`sync_levels_controls`]).
#[must_use]
pub fn levels_controls_shown(tree: &WidgetTree<WidgetKind>, controls: LevelsControls) -> bool {
    tree.style(controls.root)
        .is_some_and(|style| style.display != Display::None)
}

/// The channel the selector shows.
///
/// # Errors
///
/// As `aurora_widgets::widgets::tab_bar_state`.
pub fn levels_selected_channel(
    tree: &WidgetTree<WidgetKind>,
    controls: LevelsControls,
) -> Result<CurvesChannel, WidgetError> {
    let index = widgets::tab_bar_state(tree, controls.channel)?.selected();
    Ok(CurvesChannel::from_index(index).unwrap_or_default())
}

/// Selects `channel` in the selector (owner-driven). Returns whether the
/// selection moved.
///
/// # Errors
///
/// As `aurora_widgets::widgets::select_tab`.
pub fn select_levels_channel(
    tree: &mut WidgetTree<WidgetKind>,
    controls: LevelsControls,
    channel: CurvesChannel,
) -> Result<bool, WidgetError> {
    let disabled = widgets::tab_bar_state(tree, controls.channel)?.is_disabled();
    if disabled {
        widgets::set_tab_bar_disabled(tree, controls.channel, false)?;
    }
    let moved = widgets::select_tab(tree, controls.channel, channel.index());
    if disabled {
        widgets::set_tab_bar_disabled(tree, controls.channel, true)?;
    }
    Ok(matches!(moved?, TabBarOutcome::Selected(_)))
}

/// Mirrors `active` into `controls`, in place — never inserting or
/// removing a widget. Returns whether anything changed (cheap enough to
/// run every event-loop iteration).
///
/// - `active` not a Levels layer (or `None`): hidden and disabled.
/// - Otherwise shown and enabled; every slider shows the selected
///   channel's value except `captured` (mid-drag the slider is the
///   source of truth), and every readout follows the document.
///
/// # Errors
///
/// Whatever the underlying widget setters return for an id that isn't the
/// widget kind `controls` says it is.
pub fn sync_levels_controls(
    tree: &mut WidgetTree<WidgetKind>,
    controls: LevelsControls,
    layers: &LayerTree,
    active: Option<LayerId>,
    captured: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    let Some(params) = active.and_then(|id| levels_params(layers, id)) else {
        return set_shown(tree, controls, false);
    };
    let mut changed = set_shown(tree, controls, true)?;
    let channel = levels_selected_channel(tree, controls)?;
    let record = channel_levels(params, channel);
    for slider in LevelsSlider::ALL {
        let (Some(&id), Some(&readout)) = (
            controls.sliders.get(slider.index()),
            controls.readouts.get(slider.index()),
        ) else {
            continue;
        };
        let value = slider.value_of(&record);
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
            Some(WidgetKind::Slider(state)) => state.value,
            _ => value,
        };
        let shown_value = if slider == LevelsSlider::Gamma {
            (shown_value * 100.0).round() / 100.0
        } else {
            shown_value.round()
        };
        changed |= widgets::set_label_text(tree, readout, &slider.readout(shown_value))?;
    }
    Ok(changed)
}

/// Whether `id` is (or lies inside) the Levels editor's column.
#[must_use]
pub fn levels_controls_contains(
    tree: &WidgetTree<WidgetKind>,
    controls: &LevelsControls,
    id: WidgetId,
) -> bool {
    tree.is_within(controls.root, id)
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::indexing_slicing)]
mod tests {
    use super::{
        LevelsSlider, channel_levels, insert_levels_controls, levels_controls_shown,
        select_levels_channel, sync_levels_controls, with_channel_levels,
    };
    use crate::curves_controls::CurvesChannel;
    use aurora_core::{LevelsChannel, LevelsParams};
    use aurora_doc::{Adjustment, LayerTree};
    use aurora_widgets::widgets::{self, WidgetKind};

    fn scales() -> aurora_theme::Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn slider_value(
        tree: &aurora_widgets::WidgetTree<WidgetKind>,
        id: aurora_widgets::WidgetId,
    ) -> f64 {
        match tree.payload(id) {
            Some(WidgetKind::Slider(state)) => state.value,
            other => unreachable!("{other:?}"),
        }
    }

    /// The editor starts hidden and disabled, shows only for an active
    /// Levels layer, follows the selected channel's record and its
    /// readouts, leaves a captured slider alone and hides again.
    #[test]
    fn the_editor_follows_the_active_levels_layer_and_selected_channel() {
        let scales = scales();
        let mut ws = crate::build_workspace(&scales);
        let controls = match insert_levels_controls(&mut ws.tree, ws.properties.root, &scales) {
            Ok(c) => c,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(!levels_controls_shown(&ws.tree, controls));
        let Some(green) = LevelsSlider::Gamma.with_value(&LevelsChannel::identity(), 2.5) else {
            unreachable!("valid");
        };
        let params = with_channel_levels(&LevelsParams::identity(), CurvesChannel::Green, green);
        let mut layers = LayerTree::new();
        let Ok(id) =
            layers.add_adjustment_layer_at("Levels 1", Adjustment::Levels(params), None, 0)
        else {
            unreachable!("adjustment layer");
        };
        // Not active: stays hidden.
        assert_eq!(
            sync_levels_controls(&mut ws.tree, controls, &layers, None, None).ok(),
            Some(false)
        );
        assert_eq!(
            sync_levels_controls(&mut ws.tree, controls, &layers, Some(id), None).ok(),
            Some(true)
        );
        assert!(levels_controls_shown(&ws.tree, controls));
        let gamma = controls.sliders[LevelsSlider::Gamma.index()];
        assert_eq!(slider_value(&ws.tree, gamma), 1.0, "RGB master is identity");
        assert_eq!(
            select_levels_channel(&mut ws.tree, controls, CurvesChannel::Green).ok(),
            Some(true)
        );
        let _ = sync_levels_controls(&mut ws.tree, controls, &layers, Some(id), None);
        assert_eq!(slider_value(&ws.tree, gamma), 2.5);
        match ws
            .tree
            .payload(controls.readouts[LevelsSlider::Gamma.index()])
        {
            Some(WidgetKind::Label(state)) => assert_eq!(state.text, "Gamma: 2.50"),
            other => unreachable!("{other:?}"),
        }
        // A captured slider keeps its (mid-drag) value.
        let _ = widgets::set_slider_value(&mut ws.tree, gamma, 4.0);
        let _ = sync_levels_controls(&mut ws.tree, controls, &layers, Some(id), Some(gamma));
        assert_eq!(slider_value(&ws.tree, gamma), 4.0);
        assert_eq!(
            sync_levels_controls(&mut ws.tree, controls, &layers, None, None).ok(),
            Some(true)
        );
        assert!(!levels_controls_shown(&ws.tree, controls));
    }

    #[test]
    fn slider_values_round_trip_and_never_produce_an_invalid_record() {
        let id = LevelsChannel::identity();
        assert_eq!(LevelsSlider::InputBlack.value_of(&id), 0.0);
        assert_eq!(LevelsSlider::InputWhite.value_of(&id), 255.0);
        assert_eq!(LevelsSlider::Gamma.value_of(&id), 1.0);
        let moved = LevelsSlider::InputBlack.with_value(&id, 40.4);
        assert_eq!(
            moved.map(|c| LevelsSlider::InputBlack.value_of(&c)),
            Some(40.0)
        );
        // Black cannot reach or pass white; white cannot reach black.
        let high = LevelsSlider::InputBlack.with_value(&id, 255.0);
        assert_eq!(
            high.map(|c| LevelsSlider::InputBlack.value_of(&c)),
            Some(254.0)
        );
        let low = LevelsSlider::InputWhite.with_value(&moved.unwrap_or(id), 0.0);
        assert_eq!(
            low.map(|c| LevelsSlider::InputWhite.value_of(&c)),
            Some(41.0)
        );
        // Gamma rounds to hundredths and clamps.
        let g = LevelsSlider::Gamma.with_value(&id, 1.234_9);
        assert_eq!(
            g.map(|c| LevelsSlider::Gamma.value_of(&c) as f32),
            Some(1.23)
        );
        assert!(LevelsSlider::Gamma.with_value(&id, 50.0).is_some());
        assert!(LevelsSlider::Gamma.with_value(&id, f64::NAN).is_none());
    }

    #[test]
    fn channel_records_are_independent() {
        let params = LevelsParams::identity();
        let Some(red) = LevelsSlider::OutputWhite
            .with_value(&channel_levels(&params, CurvesChannel::Red), 128.0)
        else {
            unreachable!("valid");
        };
        let next = with_channel_levels(&params, CurvesChannel::Red, red);
        assert_eq!(next.red, red);
        assert!(next.master.is_identity() && next.green.is_identity() && next.blue.is_identity());
    }
}
