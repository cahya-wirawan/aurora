//! The Properties panel's **Curves editor** (0.156.0): a channel selector
//! (a tab bar — RGB / Red / Green / Blue) above the curve editor
//! (`aurora_widgets::widgets::insert_curve_editor`), shown only while
//! the active layer is a Curves adjustment. PLAN.md M1.8 / the 0.155.0
//! Curves adjustment's missing editor.
//!
//! **Where it lives.** Inside the Properties panel's tool-controls strip
//! ([`crate::insert_tool_controls`] builds it as the strip's last child),
//! so it survives every Properties-body repopulation, collapses with the
//! panel, and every pointer, key and accessibility event on it reaches
//! `aurora-app` through the same `ToolControls` routing the radius
//! slider already uses — no second owner threaded through every router.
//! It starts hidden (`Display::None`) and disabled.
//!
//! **This module knows nothing about undo or pixels.** It builds the
//! widgets, names the channels, maps a channel to its curve inside a
//! [`CurvesParams`] ([`channel_curve`]/[`with_channel_curve`]) and
//! mirrors a document into the widgets ([`sync_curves_controls`]).
//! Turning an editor outcome into a document edit, coalescing a drag
//! into one undo step and computing the histogram are `aurora-app`'s job.
//!
//! **An absent per-channel curve shows as the identity**, and editing it
//! makes it present — a `CurvesParams` with `Some(identity)` there
//! applies exactly like one with `None` (`aurora_filters::CurvesLut`).

use aurora_core::{CurvesParams, ToneCurve};
use aurora_doc::{Adjustment, LayerId, LayerTree};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, CurveEditorOutcome, TabBarOutcome, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{Display, FlexDirection, Size, Style};

use crate::gallery_panel::gallery_editor_size;

/// One channel the selector offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum CurvesChannel {
    /// The composite curve, applied to every colour channel.
    #[default]
    Rgb,
    /// The red channel's own curve.
    Red,
    /// The green channel's own curve.
    Green,
    /// The blue channel's own curve.
    Blue,
}

impl CurvesChannel {
    /// Every channel, in tab order — the one place that order is fixed,
    /// so a tab index and `ALL[index]` can never drift apart.
    pub const ALL: [Self; 4] = [Self::Rgb, Self::Red, Self::Green, Self::Blue];

    /// The tab's label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rgb => "RGB",
            Self::Red => "Red",
            Self::Green => "Green",
            Self::Blue => "Blue",
        }
    }

    /// The tab index of `self`.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Rgb => 0,
            Self::Red => 1,
            Self::Green => 2,
            Self::Blue => 3,
        }
    }

    /// The channel at tab `index`, `None` past the last.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

/// The channel's curve inside `params` — an absent per-channel curve is
/// the identity.
#[must_use]
pub fn channel_curve(params: &CurvesParams, channel: CurvesChannel) -> ToneCurve {
    let own = match channel {
        CurvesChannel::Rgb => return params.composite.clone(),
        CurvesChannel::Red => params.red.as_ref(),
        CurvesChannel::Green => params.green.as_ref(),
        CurvesChannel::Blue => params.blue.as_ref(),
    };
    own.cloned().unwrap_or_else(ToneCurve::identity)
}

/// `params` with `channel`'s curve replaced by `curve`, every other
/// curve untouched.
#[must_use]
pub fn with_channel_curve(
    params: &CurvesParams,
    channel: CurvesChannel,
    curve: ToneCurve,
) -> CurvesParams {
    let mut next = params.clone();
    match channel {
        CurvesChannel::Rgb => next.composite = curve,
        CurvesChannel::Red => next.red = Some(curve),
        CurvesChannel::Green => next.green = Some(curve),
        CurvesChannel::Blue => next.blue = Some(curve),
    }
    next
}

/// `id`'s Curves parameters, `None` for anything that is not a Curves
/// adjustment layer (or not in `layers` at all).
#[must_use]
pub fn curves_params(layers: &LayerTree, id: LayerId) -> Option<&CurvesParams> {
    match layers.adjustment(id) {
        Some(Adjustment::Curves(params)) => Some(params),
        _ => None,
    }
}

/// The selector and the editor, plus the column that holds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurvesControls {
    /// The column container — the tool-controls strip's last child.
    pub root: WidgetId,
    /// The channel tab bar; tab `i` is `CurvesChannel::ALL[i]`.
    pub channel: WidgetId,
    /// The curve editor, bound to the selected channel's curve.
    pub editor: WidgetId,
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

/// Adds the hidden, disabled Curves editor as the last child of
/// `parent`. The editor's side is the Widget Gallery's own square-editor
/// size ([`gallery_editor_size`]) — no "curve editor size" token exists.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist. On
/// any failure the partly built column is removed again.
pub fn insert_curves_controls(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
) -> Result<CurvesControls, WidgetError> {
    let root = widgets::insert_container(tree, parent, column_style(scales))?;
    let built = (|| {
        let channel = widgets::insert_tab_bar(
            tree,
            root,
            scales,
            "Curves channel",
            CurvesChannel::ALL
                .iter()
                .map(|channel| channel.label().to_owned())
                .collect(),
            CurvesChannel::Rgb.index(),
        )?;
        let editor = widgets::insert_curve_editor(
            tree,
            root,
            scales,
            "Curves",
            gallery_editor_size(scales),
            ToneCurve::identity(),
        )?;
        let controls = CurvesControls {
            root,
            channel,
            editor,
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
    controls: CurvesControls,
    enabled: bool,
) -> Result<bool, WidgetError> {
    let mut changed = false;
    if widgets::tab_bar_state(tree, controls.channel)?.is_disabled() == enabled {
        widgets::set_tab_bar_disabled(tree, controls.channel, !enabled)?;
        changed = true;
    }
    if widgets::curve_editor_state(tree, controls.editor)?.disabled() == enabled {
        widgets::set_curve_editor_disabled(tree, controls.editor, !enabled)?;
        changed = true;
    }
    Ok(changed)
}

fn set_shown(
    tree: &mut WidgetTree<WidgetKind>,
    controls: CurvesControls,
    shown: bool,
) -> Result<bool, WidgetError> {
    let display = if shown { Display::Flex } else { Display::None };
    let Some(mut style) = tree.style(controls.root).cloned() else {
        return Err(WidgetError::UnknownWidget(controls.root));
    };
    let mut changed = false;
    if style.display != display {
        style.display = display;
        tree.set_style(controls.root, style)?;
        changed = true;
    }
    Ok(set_enabled(tree, controls, shown)? || changed)
}

/// Whether the Curves editor is shown (the active layer is a Curves
/// adjustment, as of the last [`sync_curves_controls`]).
#[must_use]
pub fn curves_controls_shown(tree: &WidgetTree<WidgetKind>, controls: CurvesControls) -> bool {
    tree.style(controls.root)
        .is_some_and(|style| style.display != Display::None)
}

/// The channel the selector shows.
///
/// # Errors
///
/// As `aurora_widgets::widgets::tab_bar_state`.
pub fn curves_selected_channel(
    tree: &WidgetTree<WidgetKind>,
    controls: CurvesControls,
) -> Result<CurvesChannel, WidgetError> {
    let index = widgets::tab_bar_state(tree, controls.channel)?.selected();
    Ok(CurvesChannel::from_index(index).unwrap_or_default())
}

/// Selects `channel` in the selector (owner-driven — a newly opened
/// document resets to RGB). Returns whether the selection moved; the
/// editor follows on the next [`sync_curves_controls`].
///
/// # Errors
///
/// As `aurora_widgets::widgets::select_tab`.
pub fn select_curves_channel(
    tree: &mut WidgetTree<WidgetKind>,
    controls: CurvesControls,
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
/// removing a widget, so held ids and a pointer capture stay valid.
/// Returns whether anything changed (cheap enough to run every
/// event-loop iteration).
///
/// - `active` not a Curves layer (or `None`): the column is hidden and
///   disabled and the histogram cleared.
/// - Otherwise it is shown and enabled, and — unless `captured` is the
///   editor (mid-drag the editor is the source of truth) — the editor
///   shows the selected channel's curve. `histogram` (or `None`) is
///   handed to the editor as is.
///
/// # Errors
///
/// Returns whatever the underlying widget setters return for an id that
/// isn't the widget kind `controls` says it is.
pub fn sync_curves_controls(
    tree: &mut WidgetTree<WidgetKind>,
    controls: CurvesControls,
    layers: &LayerTree,
    active: Option<LayerId>,
    captured: Option<WidgetId>,
    histogram: Option<&[f32]>,
) -> Result<bool, WidgetError> {
    let Some(params) = active.and_then(|id| curves_params(layers, id)) else {
        let hidden = set_shown(tree, controls, false)?;
        let cleared = widgets::set_curve_editor_histogram(tree, controls.editor, None)?;
        return Ok(hidden || cleared == CurveEditorOutcome::Changed);
    };
    let mut changed = set_shown(tree, controls, true)?;
    if captured != Some(controls.editor) {
        let channel = curves_selected_channel(tree, controls)?;
        let curve = channel_curve(params, channel);
        changed |= widgets::set_curve_editor_points(tree, controls.editor, curve.points())?
            == CurveEditorOutcome::Changed;
    }
    changed |= widgets::set_curve_editor_histogram(tree, controls.editor, histogram)?
        == CurveEditorOutcome::Changed;
    Ok(changed)
}

/// Whether `id` is (or lies inside) the Curves editor's column.
#[must_use]
pub fn curves_controls_contains(
    tree: &WidgetTree<WidgetKind>,
    controls: &CurvesControls,
    id: WidgetId,
) -> bool {
    tree.is_within(controls.root, id)
}

#[cfg(test)]
mod tests {
    use super::{
        CurvesChannel, CurvesControls, channel_curve, curves_controls_shown,
        curves_selected_channel, insert_curves_controls, select_curves_channel,
        sync_curves_controls, with_channel_curve,
    };
    use aurora_core::{CurvePoint, CurvesParams, ToneCurve};
    use aurora_doc::{Adjustment, LayerId, LayerTree};
    use aurora_theme::Scales;
    use aurora_widgets::widgets::{self, WidgetKind};
    use aurora_widgets::{WidgetError, WidgetTree};

    fn ok<T>(result: Result<T, WidgetError>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn scales() -> Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn lifted() -> ToneCurve {
        match ToneCurve::new(&[
            CurvePoint::new(0.0, 0.0),
            CurvePoint::new(0.5, 0.75),
            CurvePoint::new(1.0, 1.0),
        ]) {
            Ok(curve) => curve,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn built() -> (WidgetTree<WidgetKind>, CurvesControls) {
        let scales = scales();
        let mut workspace = crate::build_workspace(&scales);
        let controls = ok(insert_curves_controls(
            &mut workspace.tree,
            workspace.properties.root,
            &scales,
        ));
        workspace.tree.compute_layout(1600.0, 900.0);
        (workspace.tree, controls)
    }

    fn doc() -> (LayerTree, LayerId, LayerId) {
        let mut layers = LayerTree::new();
        let pixel = match layers.add_pixel_layer(
            "a",
            aurora_core::Rect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
            None,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let params = CurvesParams {
            red: Some(lifted()),
            ..CurvesParams::identity()
        };
        let curves =
            match layers.add_adjustment_layer_at("Curves 1", Adjustment::Curves(params), None, 0) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
        (layers, pixel, curves)
    }

    fn shown_points(tree: &WidgetTree<WidgetKind>, controls: CurvesControls) -> Vec<CurvePoint> {
        ok(widgets::curve_editor_state(tree, controls.editor))
            .curve()
            .points()
            .to_vec()
    }

    #[test]
    fn an_absent_channel_curve_is_the_identity_and_editing_it_makes_it_present() {
        let params = CurvesParams::identity();
        for channel in CurvesChannel::ALL {
            assert_eq!(channel_curve(&params, channel), ToneCurve::identity());
            assert_eq!(CurvesChannel::from_index(channel.index()), Some(channel));
        }
        let next = with_channel_curve(&params, CurvesChannel::Green, lifted());
        assert_eq!(next.green, Some(lifted()));
        assert_eq!((next.red, next.blue), (None, None));
        assert_eq!(next.composite, ToneCurve::identity());
        let rgb = with_channel_curve(&params, CurvesChannel::Rgb, lifted());
        assert_eq!(rgb.composite, lifted());
        assert_eq!(rgb.green, None);
    }

    #[test]
    fn the_editor_starts_hidden_and_disabled() {
        let (tree, controls) = built();
        assert!(!curves_controls_shown(&tree, controls));
        assert!(ok(widgets::curve_editor_state(&tree, controls.editor)).disabled());
        assert!(ok(widgets::tab_bar_state(&tree, controls.channel)).is_disabled());
        assert_eq!(tree.bounds(controls.editor).map(|b| b.width), Some(0));
    }

    #[test]
    fn sync_shows_the_selected_channel_for_a_curves_layer_and_hides_otherwise() {
        let (mut tree, controls) = built();
        let (layers, pixel, curves) = doc();
        let bins = vec![1.0_f32; 256];
        assert!(ok(sync_curves_controls(
            &mut tree,
            controls,
            &layers,
            Some(curves),
            None,
            Some(&bins)
        )));
        assert!(curves_controls_shown(&tree, controls));
        assert!(!ok(widgets::curve_editor_state(&tree, controls.editor)).disabled());
        assert_eq!(
            shown_points(&tree, controls),
            ToneCurve::identity().points()
        );
        assert_eq!(
            ok(widgets::curve_editor_state(&tree, controls.editor)).histogram(),
            Some(bins.as_slice())
        );
        assert!(ok(select_curves_channel(
            &mut tree,
            controls,
            CurvesChannel::Red
        )));
        assert_eq!(
            ok(curves_selected_channel(&tree, controls)),
            CurvesChannel::Red
        );
        assert!(ok(sync_curves_controls(
            &mut tree,
            controls,
            &layers,
            Some(curves),
            None,
            Some(&bins)
        )));
        assert_eq!(shown_points(&tree, controls), lifted().points());
        assert!(
            !ok(sync_curves_controls(
                &mut tree,
                controls,
                &layers,
                Some(curves),
                None,
                Some(&bins)
            )),
            "an echo changes nothing"
        );
        // A captured editor keeps its own points.
        assert!(ok(select_curves_channel(
            &mut tree,
            controls,
            CurvesChannel::Blue
        )));
        let _ = ok(sync_curves_controls(
            &mut tree,
            controls,
            &layers,
            Some(curves),
            Some(controls.editor),
            Some(&bins),
        ));
        assert_eq!(shown_points(&tree, controls), lifted().points());
        // A pixel layer (or none) hides, disables and clears.
        for active in [Some(pixel), None] {
            let _ = ok(sync_curves_controls(
                &mut tree,
                controls,
                &layers,
                active,
                None,
                Some(&bins),
            ));
            assert!(!curves_controls_shown(&tree, controls));
            let state = ok(widgets::curve_editor_state(&tree, controls.editor));
            assert!(state.disabled());
            assert_eq!(state.histogram(), None);
        }
    }
}
