//! The app's half of the Vibrance, Exposure, Photo Filter (0.239.0), Black &
//! White and Color Balance (0.240.0) adjustment layers: the New Layer commands, the Properties editors' edits
//! (one undo step per slider drag, per key or assistive-technology value,
//! per Preserve Luminosity toggle) and the Photo Filter preset commands.
//!
//! The pattern is the Hue/Saturation editor's (`lib.rs`, 0.203.0) made
//! generic over the stored [`aurora_doc::Adjustment`]: a drag applies each
//! move to the tree directly and remembers the layer's adjustment from
//! before it ([`PendingAdjust`]); the whole gesture becomes one step when
//! the slider lets go of the pointer ([`finish`]).

use crate::{
    CompositeInvalidation, CurvesEdit, LayerCommandContext, PointerOutcome, UndoKind,
    layer_ids_in_order, new_layer_placement, refresh_history_panel, refresh_layer_row,
};
use aurora_doc::{Adjustment, LayerId, LayerTree};
use aurora_ui::AdjustKind;
use aurora_widgets::WidgetId;

/// A live slider drag: the layer it started on and that layer's
/// adjustment before it, so the whole gesture is **one** undo step.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingAdjust {
    layer: LayerId,
    start: Adjustment,
}

// -- New layer -------------------------------------------------------------

/// The layer name stem and the adjustment a new layer of `kind` holds.
/// Vibrance, Exposure and Color Balance start neutral; Photo Filter starts
/// as Photoshop's own default (Warming Filter (85), density 25, Preserve
/// Luminosity on) and Black & White at Photoshop's six default values.
fn new_adjustment(kind: AdjustKind) -> (&'static str, Adjustment) {
    match kind {
        AdjustKind::Vibrance => (
            "Vibrance",
            Adjustment::Vibrance(aurora_core::VibranceParams::identity()),
        ),
        AdjustKind::Exposure => (
            "Exposure",
            Adjustment::Exposure(aurora_core::ExposureParams::identity()),
        ),
        AdjustKind::PhotoFilter => (
            "Photo Filter",
            Adjustment::PhotoFilter(aurora_core::PhotoFilterParams::default()),
        ),
        // Black & White starts at Photoshop's defaults (it is never
        // neutral); Color Balance starts neutral with Preserve Luminosity on.
        AdjustKind::BlackWhite => (
            "Black & White",
            Adjustment::BlackWhite(aurora_core::BlackWhiteParams::default()),
        ),
        // 0.244.0: Invert has no settings; Threshold starts at Photoshop's
        // default 128, Posterize at its default 4 levels; Brightness/Contrast
        // starts neutral in the default mode.
        AdjustKind::Invert => ("Invert", Adjustment::Invert),
        AdjustKind::Threshold => (
            "Threshold",
            Adjustment::Threshold(aurora_core::ThresholdParams::default()),
        ),
        AdjustKind::Posterize => (
            "Posterize",
            Adjustment::Posterize(aurora_core::PosterizeParams::default()),
        ),
        AdjustKind::BrightnessContrast => (
            "Brightness/Contrast",
            Adjustment::BrightnessContrast(aurora_core::BrightnessContrastParams::identity()),
        ),
        // 0.245.0: Channel Mixer starts as the identity (each output its own
        // source at 100 %), Selective Color neutral and Relative.
        AdjustKind::ChannelMixer => (
            "Channel Mixer",
            Adjustment::ChannelMixer(aurora_core::ChannelMixerParams::identity()),
        ),
        AdjustKind::SelectiveColor => (
            "Selective Color",
            Adjustment::SelectiveColor(aurora_core::SelectiveColorParams::identity()),
        ),
        // 0.248.0: Gradient Map starts as black to white (a greyscale
        // conversion, not an identity).
        AdjustKind::GradientMap => (
            "Gradient Map",
            Adjustment::GradientMap(aurora_core::GradientMapParams::black_to_white()),
        ),
        AdjustKind::ColorBalance => (
            "Color Balance",
            Adjustment::ColorBalance(aurora_core::ColorBalanceParams::identity()),
        ),
    }
}

/// `"<stem> N"`, one past the highest `N` any existing layer already uses.
fn next_name(layers: &LayerTree, stem: &str) -> String {
    let prefix = format!("{stem} ");
    let highest = layer_ids_in_order(layers)
        .into_iter()
        .filter_map(|id| {
            layers
                .name(id)?
                .strip_prefix(prefix.as_str())?
                .parse::<u64>()
                .ok()
        })
        .max()
        .unwrap_or(0);
    format!("{prefix}{}", highest.saturating_add(1))
}

/// New Vibrance / Exposure / Photo Filter Layer: an adjustment directly
/// above the active node, placed exactly where New Levels Layer places
/// one; one undo step.
pub(crate) fn new_layer(cx: &mut LayerCommandContext<'_>, kind: AdjustKind) -> Option<LayerId> {
    let placement = new_layer_placement(cx.layers, *cx.active_layer);
    let (stem, adjustment) = new_adjustment(kind);
    let name = next_name(cx.layers, stem);
    match cx.history.add_adjustment_layer_at(
        cx.layers,
        name,
        adjustment,
        placement.parent,
        placement.index,
    ) {
        Ok(id) => Some(id),
        Err(err) => {
            tracing::warn!(?err, "New {stem} Layer was refused by the layer tree");
            None
        }
    }
}

// -- The Properties editors ------------------------------------------------

/// Whether `outcome` belongs to the Vibrance / Exposure / Photo Filter
/// editors: one of their sliders or the Preserve Luminosity checkbox.
#[allow(clippy::large_types_passed_by_value)] // `CurvesControls` is `Copy` and 448 bytes
pub(crate) fn is_outcome(controls: aurora_ui::CurvesControls, outcome: &PointerOutcome) -> bool {
    use aurora_widgets::ActionOutcome as Outcome;
    let Some(adjust) = controls.adjust else {
        return false;
    };
    match outcome {
        PointerOutcome::Action(Outcome::ValueChanged { id, .. }) => adjust.slider_of(*id).is_some(),
        // The Gradient Map stop editor (task 113): a marker drag, a marker
        // press (selection only) and the four stop buttons.
        PointerOutcome::Action(Outcome::GradientBarChanged { bar })
        | PointerOutcome::Changed(bar) => *bar == adjust.gradient_bar,
        PointerOutcome::Action(Outcome::Activated(id)) => gradient_button(adjust, *id).is_some(),
        PointerOutcome::Action(Outcome::Toggled { id, .. }) => {
            *id == adjust.preserve
                || *id == adjust.tint
                || *id == adjust.balance_preserve
                || *id == adjust.legacy
                || *id == adjust.mixer_mono
                || *id == adjust.selective_absolute
                || *id == adjust.gradient_reverse
        }
        // The Color Balance range selector (0.240.0): widget state only, no
        // document edit, but a pending drag settles when it is used.
        PointerOutcome::Action(Outcome::TabSelected { bar, .. }) => {
            *bar == adjust.balance_range
                || *bar == adjust.mixer_output
                || *bar == adjust.selective_group
                || *bar == adjust.selective_range
        }
        _ => false,
    }
}

/// What a Gradient Map stop button does (task 113).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GradientButton {
    Previous,
    Next,
    Add,
    Delete,
}

/// The stop button `id` is, if any.
#[allow(clippy::large_types_passed_by_value)] // `AdjustControls` is `Copy`
fn gradient_button(adjust: aurora_ui::AdjustControls, id: WidgetId) -> Option<GradientButton> {
    [
        (adjust.gradient_prev, GradientButton::Previous),
        (adjust.gradient_next, GradientButton::Next),
        (adjust.gradient_add, GradientButton::Add),
        (adjust.gradient_delete, GradientButton::Delete),
    ]
    .into_iter()
    .find_map(|(button, which)| (button == id).then_some(which))
}

/// Whether the pointer capture is on one of the editors' sliders or the
/// Gradient Map bar's marker.
#[allow(clippy::large_types_passed_by_value)] // `CurvesControls` is `Copy` and 448 bytes
pub(crate) fn slider_captured(
    controls: Option<aurora_ui::CurvesControls>,
    captured: Option<WidgetId>,
) -> bool {
    controls
        .and_then(|controls| controls.adjust)
        .zip(captured)
        .is_some_and(|(adjust, id)| adjust.slider_of(id).is_some() || id == adjust.gradient_bar)
}

/// Records one edit as a structural step.
fn record_step(cx: &mut CurvesEdit<'_>, id: LayerId, adjustment: Adjustment) -> bool {
    if let Err(err) = cx.history.set_adjustment(cx.layers, id, adjustment) {
        tracing::warn!(?err, "an adjustment edit was refused by the layer tree");
        return false;
    }
    cx.undo_order
        .record(UndoKind::Structural, cx.history, cx.pixel_history);
    refresh_history_panel(cx.workspace, cx.undo_order);
    refresh_layer_row(cx.workspace, cx.layer_rows, cx.layers, id);
    true
}

/// Ends a live drag as one undo step: the live edits are rewound, then
/// start -> final is recorded. Returns whether a step was recorded.
pub(crate) fn finish(cx: &mut CurvesEdit<'_>) -> bool {
    let Some(pending) = cx.state.adjust_pending.take() else {
        return false;
    };
    let Some(now) = aurora_ui::adjust_of(cx.layers, pending.layer).cloned() else {
        return false;
    };
    if now == pending.start {
        return false;
    }
    if let Err(err) = cx.layers.set_adjustment(pending.layer, pending.start) {
        tracing::warn!(
            ?err,
            "failed to rewind an adjustment drag before recording it"
        );
        return false;
    }
    record_step(cx, pending.layer, now)
}

/// Commits a pending drag once no slider holds the pointer capture.
pub(crate) fn settle(cx: &mut CurvesEdit<'_>, captured: Option<WidgetId>) -> bool {
    if slider_captured(cx.controls, captured) {
        return false;
    }
    cx.state.adjust_detached = false;
    cx.state.adjust_pending.is_some() && finish(cx)
}

/// Turns one editor outcome into a document edit (the Color Balance range
/// selector is no edit: the next sync rebinds its sliders): a slider with the
/// pointer captured is a live drag (one step when it ends); a slider
/// without capture (an arrow key, an assistive technology) and a checkbox
/// toggle are one step each. Every real change is `Everything`.
#[allow(clippy::too_many_lines)] // one arm per control of the editors
pub(crate) fn apply_outcome(
    cx: &mut CurvesEdit<'_>,
    outcome: &PointerOutcome,
    captured: Option<WidgetId>,
) -> CompositeInvalidation {
    let Some(adjust) = cx.controls.and_then(|controls| controls.adjust) else {
        return CompositeInvalidation::None;
    };
    let _ = settle(cx, captured);
    let PointerOutcome::Action(action) = outcome else {
        return CompositeInvalidation::None;
    };
    let Some(active) = cx.active_layer.filter(|&id| cx.layers.contains(id)) else {
        return CompositeInvalidation::None;
    };
    let Some(current) = aurora_ui::adjust_of(cx.layers, active).cloned() else {
        return CompositeInvalidation::None;
    };
    let (next, dragged) = match action {
        aurora_widgets::ActionOutcome::Toggled { id, state } if *id == adjust.preserve => {
            let Some(filter) = current.photo_filter() else {
                return CompositeInvalidation::None;
            };
            let on = *state == accesskit::Toggled::True;
            (
                Adjustment::PhotoFilter(filter.with_preserve_luminosity(on)),
                None,
            )
        }
        aurora_widgets::ActionOutcome::Toggled { id, state } if *id == adjust.tint => {
            let Some(bw) = current.black_white() else {
                return CompositeInvalidation::None;
            };
            let on = *state == accesskit::Toggled::True;
            (Adjustment::BlackWhite(bw.with_tint(on)), None)
        }
        aurora_widgets::ActionOutcome::Toggled { id, state } if *id == adjust.balance_preserve => {
            let Some(balance) = current.color_balance() else {
                return CompositeInvalidation::None;
            };
            let on = *state == accesskit::Toggled::True;
            (
                Adjustment::ColorBalance(balance.with_preserve_luminosity(on)),
                None,
            )
        }
        aurora_widgets::ActionOutcome::Toggled { id, state } if *id == adjust.legacy => {
            let Some(bc) = current.brightness_contrast() else {
                return CompositeInvalidation::None;
            };
            let on = *state == accesskit::Toggled::True;
            (Adjustment::BrightnessContrast(bc.with_legacy(on)), None)
        }
        aurora_widgets::ActionOutcome::Toggled { id, state } if *id == adjust.mixer_mono => {
            let Some(mixer) = current.channel_mixer() else {
                return CompositeInvalidation::None;
            };
            let on = *state == accesskit::Toggled::True;
            (Adjustment::ChannelMixer(mixer.with_monochrome(on)), None)
        }
        aurora_widgets::ActionOutcome::Toggled { id, state }
            if *id == adjust.selective_absolute =>
        {
            let Some(selective) = current.selective_color() else {
                return CompositeInvalidation::None;
            };
            let on = *state == accesskit::Toggled::True;
            (
                Adjustment::SelectiveColor(selective.with_absolute(on)),
                None,
            )
        }
        aurora_widgets::ActionOutcome::Toggled { id, state } if *id == adjust.gradient_reverse => {
            let Some(gradient) = current.gradient_map() else {
                return CompositeInvalidation::None;
            };
            let on = *state == accesskit::Toggled::True;
            (Adjustment::GradientMap(gradient.with_reverse(on)), None)
        }
        aurora_widgets::ActionOutcome::Activated(id) => {
            let Some(button) = gradient_button(adjust, *id) else {
                return CompositeInvalidation::None;
            };
            let Some(gradient) = current.gradient_map() else {
                return CompositeInvalidation::None;
            };
            let tree = &mut cx.workspace.tree;
            let selected = aurora_ui::adjust_gradient_selected(tree, adjust);
            let count = gradient.stops().len();
            // Selecting is widget state: no undo step, never dirty.
            let select = |tree: &mut aurora_widgets::WidgetTree<_>, to: usize| {
                if let Err(err) =
                    aurora_widgets::widgets::select_gradient_stop(tree, adjust.gradient_bar, to)
                {
                    tracing::warn!(?err, "a Gradient Map stop could not be selected");
                }
            };
            let (next, reselect) = match button {
                GradientButton::Previous => {
                    if selected > 0 {
                        select(tree, selected - 1);
                    }
                    return CompositeInvalidation::None;
                }
                GradientButton::Next => {
                    if selected + 1 < count {
                        select(tree, selected + 1);
                    }
                    return CompositeInvalidation::None;
                }
                GradientButton::Add => {
                    // Between the selected stop and the one after it; for the
                    // last stop, between it and the one before.
                    let after = if selected + 1 < count {
                        selected
                    } else {
                        count.saturating_sub(2)
                    };
                    match gradient.insert_stop_after(after) {
                        Ok((next, at)) => (next, at),
                        Err(_) => return CompositeInvalidation::None,
                    }
                }
                GradientButton::Delete => match gradient.remove_stop(selected) {
                    Ok(next) => (next, selected.saturating_sub(1)),
                    Err(_) => return CompositeInvalidation::None,
                },
            };
            // The bar takes the new stops first so the selection can move
            // onto a stop it did not have.
            let stops: Vec<_> = next
                .stops()
                .iter()
                .map(|s| aurora_widgets::widgets::GradientBarStop::new(s.position, s.colour))
                .collect();
            if let Err(err) =
                aurora_widgets::widgets::set_gradient_bar_stops(tree, adjust.gradient_bar, &stops)
            {
                tracing::warn!(?err, "the Gradient Map bar could not take the new stops");
            }
            select(tree, reselect);
            (Adjustment::GradientMap(next), None)
        }
        aurora_widgets::ActionOutcome::GradientBarChanged { bar }
            if *bar == adjust.gradient_bar =>
        {
            let Some(gradient) = current.gradient_map() else {
                return CompositeInvalidation::None;
            };
            let Ok(state) = aurora_widgets::widgets::gradient_bar_state(&cx.workspace.tree, *bar)
            else {
                return CompositeInvalidation::None;
            };
            let selected = state.selected();
            let Some(position) = state.stops().get(selected).map(|s| s.position) else {
                return CompositeInvalidation::None;
            };
            let Ok(next) = gradient.with_stop_position(selected, position) else {
                return CompositeInvalidation::None;
            };
            (Adjustment::GradientMap(next), Some(*bar))
        }
        aurora_widgets::ActionOutcome::ValueChanged { id, value } => {
            let Some(slider) = adjust.slider_of(*id) else {
                return CompositeInvalidation::None;
            };
            let range = aurora_ui::adjust_tab(&cx.workspace.tree, adjust, slider.kind());
            let Some(next) = slider.with_value(&current, range, *value) else {
                return CompositeInvalidation::None;
            };
            (next, Some(*id))
        }
        _ => return CompositeInvalidation::None,
    };
    if dragged.is_some() && captured == dragged {
        if cx
            .state
            .adjust_pending
            .as_ref()
            .is_some_and(|pending| pending.layer != active)
        {
            let _ = finish(cx);
            cx.state.adjust_detached = true;
        }
        if cx.state.adjust_detached || next == current {
            return CompositeInvalidation::None;
        }
        if cx.state.adjust_pending.is_none() {
            cx.state.adjust_pending = Some(PendingAdjust {
                layer: active,
                start: current,
            });
        }
        return match cx.layers.set_adjustment(active, next) {
            Ok(()) => CompositeInvalidation::Everything,
            Err(err) => {
                tracing::warn!(?err, "a live adjustment drag move was refused");
                CompositeInvalidation::None
            }
        };
    }
    if next == current || !record_step(cx, active, next) {
        return CompositeInvalidation::None;
    }
    CompositeInvalidation::Everything
}

/// Mirrors the document into the editors (run every loop turn); returns
/// whether anything shown changed.
#[allow(clippy::large_types_passed_by_value)] // `AdjustControls` is `Copy` and 400 bytes
pub(crate) fn sync(
    tree: &mut aurora_widgets::WidgetTree<aurora_widgets::widgets::WidgetKind>,
    controls: Option<aurora_ui::AdjustControls>,
    layers: &LayerTree,
    active: Option<LayerId>,
    captured: Option<WidgetId>,
) -> bool {
    let Some(adjust) = controls else {
        return false;
    };
    match aurora_ui::sync_adjust_controls(tree, adjust, layers, active, captured) {
        Ok(changed) => changed,
        Err(err) => {
            tracing::warn!(
                ?err,
                "failed to sync the Vibrance/Exposure/Photo Filter/Black & White/Color Balance editors"
            );
            false
        }
    }
}

// -- Photo Filter presets --------------------------------------------------

/// The command id of preset `index` of [`aurora_core::PHOTO_FILTER_PRESETS`].
pub(crate) fn preset_command_id(index: usize) -> String {
    format!("photo_filter.preset.{index}")
}

/// The palette title of preset `index`.
pub(crate) fn preset_command_title(index: usize) -> String {
    let name = aurora_core::PHOTO_FILTER_PRESETS
        .get(index)
        .map_or("", |preset| preset.0);
    format!("Photo Filter: {name}")
}

/// The preset a command id names.
pub(crate) fn preset_for_command(id: &str) -> Option<usize> {
    (0..aurora_core::PHOTO_FILTER_PRESETS.len()).find(|&i| preset_command_id(i) == id)
}

/// Sets the active Photo Filter layer's colour to preset `index`, keeping
/// its density and Preserve Luminosity: one undo step. `false` (nothing
/// changed) when the active layer is not a Photo Filter, the preset does
/// not exist or the colour is already that preset's.
pub(crate) fn apply_preset(cx: &mut CurvesEdit<'_>, index: usize) -> bool {
    let Some(&(_, rgb)) = aurora_core::PHOTO_FILTER_PRESETS.get(index) else {
        return false;
    };
    let Some(active) = cx.active_layer.filter(|&id| cx.layers.contains(id)) else {
        return false;
    };
    let Some(filter) = cx
        .layers
        .adjustment(active)
        .and_then(Adjustment::photo_filter)
    else {
        return false;
    };
    let colour = rgb.map(|c| f32::from(c) / 255.0);
    let Ok(next) = filter.with_colour(colour) else {
        return false;
    };
    if next == filter {
        return false;
    }
    // A live drag on the same layer ends first, as its own step.
    let _ = finish(cx);
    record_step(cx, active, Adjustment::PhotoFilter(next))
}
