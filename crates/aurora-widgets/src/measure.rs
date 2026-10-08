//! Text-aware layout (0.140.0): sizing a widget from its own text before
//! layout runs.
//!
//! [`WidgetTree::compute_layout`] is text-blind — it knows only each
//! widget's style. [`compute_text_layout`] lays the same tree out through
//! [`WidgetTree::compute_layout_with`], asking [`measure_widget`] for each
//! widget's size first. Today exactly one kind is measured: a `Checkbox`
//! with a non-blank label becomes its box, a `spacing.sm` gap and its
//! label, one line tall. Every other widget, and every checkbox without a
//! text engine to measure with, keeps its style's size — which is why the
//! headless goldens (laid out with no engine) are unchanged.
//!
//! One line only: no wrapping, no ellipsis. A label wider than the space
//! its parent offers overflows (and is clipped by any clipping ancestor);
//! wrapping is deferred work, recorded in PLAN.md.

use aurora_text::TextEngine;
use aurora_theme::Scales;
use taffy::Size;

use crate::text::{label_style, sanitize_label};
use crate::tree::WidgetTree;
use crate::widgets::{
    WidgetKind, checkbox_metrics, row_height, settle_linked_scrollbars, sync_linked_scrollbars,
};

/// What measuring needs: the application's one text engine, the token
/// scales the widgets were built from, and the display's scale factor —
/// the same one the frame's text is painted at, so the shaped lines a
/// layout pass measures are the very cache entries the paint that follows
/// reuses.
#[derive(Debug)]
pub struct TextMeasure<'a> {
    /// The shared text engine (its shape cache is filled as a side effect).
    pub engine: &'a mut TextEngine,
    /// The token scales every widget's own style came from.
    pub scales: &'a Scales,
    /// Physical pixels per logical pixel.
    pub scale_factor: f32,
}

/// The size `kind` should be laid out at, logical px, or `None` to keep
/// its style's own size.
///
/// A `Checkbox` whose sanitized label is not blank measures as
/// `ceil(side + gap + label width)` wide (`side` = `type.size.md`, `gap` =
/// `spacing.sm`; rounded *up* because layout truncates bounds to whole
/// pixels, and a truncated box would clip the label's last glyph) and
/// `max(row_height, side)` tall. A blank or control-only label, or a
/// width that shapes to nothing finite and positive (e.g. an invalid
/// scale factor), measures as `None`: the checkbox stays a bare box.
#[must_use]
pub fn measure_widget(kind: &WidgetKind, measure: &mut TextMeasure<'_>) -> Option<Size<f32>> {
    let WidgetKind::Checkbox(state) = kind else {
        return None;
    };
    let text = sanitize_label(&state.label);
    if text.trim().is_empty() {
        return None;
    }
    let (side, gap) = checkbox_metrics(measure.scales);
    let line = measure
        .engine
        .shape(&text, &label_style(measure.scales), measure.scale_factor);
    let width = line.width;
    if !width.is_finite() || width <= 0.0 {
        return None;
    }
    Some(Size {
        width: (side + gap + width).ceil(),
        height: row_height(measure.scales).max(side),
    })
}

/// Lays `tree` out in `width` x `height` (logical px), text-aware when a
/// [`TextMeasure`] is supplied ([`measure_widget`] sizes each widget
/// first) and exactly [`WidgetTree::compute_layout`] when it is `None` —
/// the one entry point a production caller uses, so a missing text engine
/// (the UI font failed to load) degrades to unlabelled boxes rather than
/// to a different code path.
///
/// **Linked scrollbars follow the layout (0.146.0).** After laying out,
/// every scrollbar linked to a scroll container
/// ([`crate::widgets::link_scrollbar`]) takes that container's new offset,
/// range and height ([`sync_linked_scrollbars`]); when one is shown or
/// hidden — which changes its container's width — the tree is laid out
/// exactly once more, never a third time, and a settling pass
/// ([`settle_linked_scrollbars`]) syncs the bars again without showing or
/// hiding any, so the drawn bars always match the final layout. For content
/// whose height does not shrink as it narrows (text, rows, every shipped
/// panel) one extra layout is always enough; content that does shrink can
/// leave a bar in its first-pass state, logged as a warning. Plain
/// [`WidgetTree::compute_layout`] does none of this.
pub fn compute_text_layout(
    tree: &mut WidgetTree<WidgetKind>,
    width: f32,
    height: f32,
    text: Option<TextMeasure<'_>>,
) {
    match text {
        None => {
            tree.compute_layout(width, height);
            if sync_linked_scrollbars(tree) {
                tree.compute_layout(width, height);
                warn_if_unsettled(settle_linked_scrollbars(tree));
            }
        }
        Some(mut measure) => {
            tree.compute_layout_with(width, height, &mut |kind| {
                measure_widget(kind, &mut measure)
            });
            if sync_linked_scrollbars(tree) {
                tree.compute_layout_with(width, height, &mut |kind| {
                    measure_widget(kind, &mut measure)
                });
                warn_if_unsettled(settle_linked_scrollbars(tree));
            }
        }
    }
}

/// The diagnostic for [`settle_linked_scrollbars`]'s one unstable case.
fn warn_if_unsettled(unsettled: bool) {
    if unsettled {
        tracing::warn!(
            "a linked scrollbar's visibility did not settle in two layouts: content \
             whose height shrinks as it narrows; the bar keeps its first-pass visibility"
        );
    }
}

#[cfg(test)]
mod tests {
    use accesskit::Toggled;
    use aurora_core::Rect;
    use aurora_text::TextEngine;
    use aurora_theme::{Palette, Scales, Theme, ThemeSet};
    use taffy::{FlexDirection, Style};

    use super::{TextMeasure, compute_text_layout, measure_widget};
    use crate::input::FocusManager;
    use crate::pointer::{ClickTracker, PointerEvent, PointerPhase, handle_pointer};
    use crate::text::{label_style, resolve_text, text_runs};
    use crate::tree::{WidgetId, WidgetTree};
    use crate::widgets::{
        WidgetKind, checkbox_metrics, insert_button, insert_checkbox, new_tree, row_height,
        set_checkbox_disabled, test_scales,
    };
    use crate::{HAlign, paint_widget, paint_widget_ops_frame};

    const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
    const DARK_THEME_TOML: &str = include_str!("../../../design/themes/dark.toml");

    fn dark_theme() -> Theme {
        let Ok(palette) = Palette::from_toml_str(PALETTE_TOML) else {
            unreachable!("the committed palette parses");
        };
        let mut themes = ThemeSet::new();
        if themes.register(DARK_THEME_TOML).is_err() {
            unreachable!("the committed Dark theme registers");
        }
        match themes.resolve("Dark", &palette) {
            Ok(theme) => theme,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn engine() -> TextEngine {
        ok(TextEngine::new())
    }

    /// A column (default `align_items`: stretch) — the shape both real
    /// checkbox parents (the gallery panel's left column, the Layers
    /// panel's control strip) have — holding a button, then one checkbox
    /// per label, then another button.
    fn column(scales: &Scales, labels: &[&str]) -> (WidgetTree<WidgetKind>, Vec<WidgetId>) {
        let (mut tree, root) = new_tree(Style {
            flex_direction: FlexDirection::Column,
            ..Default::default()
        });
        ok(insert_button(&mut tree, root, scales, "Before"));
        let ids = labels
            .iter()
            .map(|label| ok(insert_checkbox(&mut tree, root, scales, *label)))
            .collect();
        ok(insert_button(&mut tree, root, scales, "After"));
        (tree, ids)
    }

    fn layout(
        tree: &mut WidgetTree<WidgetKind>,
        engine: &mut TextEngine,
        scales: &Scales,
        sf: f32,
    ) {
        compute_text_layout(
            tree,
            400.0,
            300.0,
            Some(TextMeasure {
                engine,
                scales,
                scale_factor: sf,
            }),
        );
    }

    fn bounds(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Rect {
        match tree.bounds(id) {
            Some(bounds) => bounds,
            None => unreachable!("known id"),
        }
    }

    fn every_bounds(tree: &WidgetTree<WidgetKind>) -> Vec<Rect> {
        let mut out = Vec::new();
        let mut stack = vec![tree.root()];
        while let Some(id) = stack.pop() {
            out.push(bounds(tree, id));
            stack.extend(tree.children(id).unwrap_or_default().iter().copied());
        }
        out
    }

    #[test]
    fn without_an_engine_text_layout_is_exactly_the_text_blind_layout() {
        let scales = test_scales();
        let (mut blind, _) = column(&scales, &["Visible", "Checked"]);
        let (mut text, _) = column(&scales, &["Visible", "Checked"]);
        blind.compute_layout(400.0, 300.0);
        compute_text_layout(&mut text, 400.0, 300.0, None);
        assert_eq!(every_bounds(&blind), every_bounds(&text));
        let root = text.root();
        assert_eq!(text.is_measured(root), Some(false));
    }

    #[test]
    fn a_checkbox_measures_as_box_gap_and_label_one_row_tall() {
        let scales = test_scales();
        let mut engine = engine();
        let (mut tree, ids) = column(&scales, &["Visible"]);
        layout(&mut tree, &mut engine, &scales, 1.0);
        let [checkbox] = ids.as_slice() else {
            unreachable!("one label");
        };
        let (side, gap) = checkbox_metrics(&scales);
        let label = engine.shape("Visible", &label_style(&scales), 1.0).width;
        assert!(label > 0.0, "the bundled font shapes a real label");
        let got = bounds(&tree, *checkbox);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let expected = (side + gap + label).ceil() as u32;
        assert_eq!(got.width, expected, "box + gap + label, rounded up");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let height = row_height(&scales).max(side) as u32;
        assert_eq!(got.height, height, "one row tall");
        assert_eq!(tree.is_measured(*checkbox), Some(true));
        // The explicit width stops the stretching column from widening it.
        assert!(got.width < 400);
    }

    /// Rounded *up*, not to nearest: `taffy` rounds a fractional size to
    /// the nearest pixel, which for a label whose fractional part is under
    /// one half would cut the box short of its last glyph's advance. The
    /// test insists at least one label lands in that half, or it could
    /// not tell `ceil` from `taffy`'s own rounding.
    #[test]
    fn every_measured_width_is_rounded_up_never_to_nearest() {
        let scales = test_scales();
        let mut engine = engine();
        let labels = [
            "Visible",
            "Checkbox",
            "Checked",
            "Unchecked",
            "Disabled",
            "Remember me",
            "fly",
            "Mjj",
            "WWW",
            "i",
        ];
        let (side, gap) = checkbox_metrics(&scales);
        let mut sensitive = 0;
        for sf in [1.0_f32, 1.25, 1.5, 2.0] {
            let (mut tree, ids) = column(&scales, &labels);
            layout(&mut tree, &mut engine, &scales, sf);
            for (label, id) in labels.iter().zip(ids) {
                let exact = side + gap + engine.shape(label, &label_style(&scales), sf).width;
                let fraction = exact - exact.floor();
                if fraction > 0.0 && fraction < 0.5 {
                    sensitive += 1;
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let expected = exact.ceil() as u32;
                assert_eq!(bounds(&tree, id).width, expected, "{label:?} at {sf}");
            }
        }
        assert!(
            sensitive > 0,
            "no label can tell ceil from round-to-nearest"
        );
    }

    #[test]
    fn a_longer_label_measures_wider_and_pushes_its_siblings_down() {
        let scales = test_scales();
        let mut engine = engine();
        let (mut tree, ids) = column(&scales, &["On", "A much longer checkbox label"]);
        let (mut blind, blind_ids) = column(&scales, &["On", "A much longer checkbox label"]);
        layout(&mut tree, &mut engine, &scales, 1.0);
        blind.compute_layout(400.0, 300.0);
        let [short, long] = ids.as_slice() else {
            unreachable!("two labels");
        };
        assert!(bounds(&tree, *long).width > bounds(&tree, *short).width);
        let [_, blind_long] = blind_ids.as_slice() else {
            unreachable!("two labels");
        };
        assert!(
            bounds(&tree, *long).y > bounds(&blind, *blind_long).y,
            "a taller measured row moves the next one down"
        );
    }

    #[test]
    fn measuring_at_scale_two_agrees_with_scale_one_within_a_pixel() {
        let scales = test_scales();
        let mut engine = engine();
        let (mut one, ids_one) = column(&scales, &["Visible", "Checkbox", "Remember me"]);
        let (mut two, ids_two) = column(&scales, &["Visible", "Checkbox", "Remember me"]);
        layout(&mut one, &mut engine, &scales, 1.0);
        layout(&mut two, &mut engine, &scales, 2.0);
        for (a, b) in ids_one.iter().zip(&ids_two) {
            let (a, b) = (bounds(&one, *a), bounds(&two, *b));
            assert!(a.width.abs_diff(b.width) <= 1, "{a:?} vs {b:?}");
            assert_eq!(a.height, b.height);
        }
    }

    #[test]
    fn a_blank_or_control_only_label_stays_a_bare_box() {
        let scales = test_scales();
        let mut engine = engine();
        let (mut tree, ids) = column(&scales, &["", "\n\t", "   "]);
        layout(&mut tree, &mut engine, &scales, 1.0);
        let side = scales.typography.size.md;
        for id in ids {
            let got = bounds(&tree, id);
            assert_eq!((got.width, got.height), (side, side), "{got:?}");
            assert_eq!(tree.is_measured(id), Some(false));
        }
    }

    #[test]
    fn only_a_checkbox_is_measured() {
        let scales = test_scales();
        let mut engine = engine();
        let (tree, _) = column(&scales, &["x"]);
        let mut measure = TextMeasure {
            engine: &mut engine,
            scales: &scales,
            scale_factor: 1.0,
        };
        let mut kinds = 0;
        let mut stack = vec![tree.root()];
        while let Some(id) = stack.pop() {
            stack.extend(tree.children(id).unwrap_or_default().iter().copied());
            let Some(kind) = tree.payload(id) else {
                unreachable!("known id");
            };
            let is_checkbox = matches!(kind, WidgetKind::Checkbox(_));
            assert_eq!(measure_widget(kind, &mut measure).is_some(), is_checkbox);
            kinds += 1;
        }
        assert_eq!(kinds, 4);
    }

    #[test]
    fn an_invalid_scale_factor_measures_nothing() {
        let scales = test_scales();
        let mut engine = engine();
        let (mut tree, ids) = column(&scales, &["Visible"]);
        layout(&mut tree, &mut engine, &scales, f32::NAN);
        for id in ids {
            assert_eq!(tree.is_measured(id), Some(false));
        }
    }

    #[test]
    fn clicking_a_measured_checkbox_label_toggles_it() {
        let scales = test_scales();
        let mut engine = engine();
        let (mut tree, ids) = column(&scales, &["Visible"]);
        layout(&mut tree, &mut engine, &scales, 1.0);
        let [checkbox] = ids.as_slice() else {
            unreachable!("one label");
        };
        let b = bounds(&tree, *checkbox);
        let (side, gap) = checkbox_metrics(&scales);
        #[allow(clippy::cast_precision_loss)]
        let point = (b.x as f32 + side + gap + 4.0, b.y as f32 + 4.0);
        assert_eq!(
            crate::hit_test(&tree, f64::from(point.0), f64::from(point.1)),
            Some(*checkbox),
            "the label is part of the checkbox"
        );
        let mut focus = FocusManager::new();
        let mut click = ClickTracker::default();
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            ok(handle_pointer(
                &mut tree,
                &mut focus,
                &mut click,
                PointerEvent {
                    phase,
                    position: point,
                },
            ));
        }
        match tree.payload(*checkbox) {
            Some(WidgetKind::Checkbox(state)) => assert_eq!(state.checked, Toggled::True),
            other => unreachable!("{other:?}"),
        }
    }

    /// The box mesh's bounding box `(x0, y0, x1, y1)`.
    fn box_extent(tree: &WidgetTree<WidgetKind>, id: WidgetId, scales: &Scales) -> [f32; 4] {
        let paints = ok(paint_widget(tree, id, &dark_theme(), scales, 1.0));
        let Some((mesh, _)) = paints.first() else {
            unreachable!("a checkbox paints its box");
        };
        let mut out = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for v in &mesh.vertices {
            out = [
                out[0].min(v.x),
                out[1].min(v.y),
                out[2].max(v.x),
                out[3].max(v.y),
            ];
        }
        out
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn a_measured_checkbox_paints_its_box_left_and_vertically_centred() {
        let scales = test_scales();
        let mut engine = engine();
        let (mut tree, ids) = column(&scales, &["Visible"]);
        layout(&mut tree, &mut engine, &scales, 1.0);
        let [checkbox] = ids.as_slice() else {
            unreachable!("one label");
        };
        let b = bounds(&tree, *checkbox);
        let (side, _) = checkbox_metrics(&scales);
        let top = b.y as f32 + (b.height as f32 - side) / 2.0;
        let got = box_extent(&tree, *checkbox, &scales);
        let expected = [b.x as f32, top, b.x as f32 + side, top + side];
        for (g, e) in got.iter().zip(expected) {
            assert!((g - e).abs() < 0.01, "{got:?} vs {expected:?}");
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn an_unmeasured_checkbox_paints_its_whole_bounds() {
        let scales = test_scales();
        let (mut tree, ids) = column(&scales, &["Visible"]);
        tree.compute_layout(400.0, 300.0);
        let [checkbox] = ids.as_slice() else {
            unreachable!("one label");
        };
        let b = bounds(&tree, *checkbox);
        let got = box_extent(&tree, *checkbox, &scales);
        let expected = [
            b.x as f32,
            b.y as f32,
            (b.x + i64::from(b.width)) as f32,
            (b.y + i64::from(b.height)) as f32,
        ];
        for (g, e) in got.iter().zip(expected) {
            assert!((g - e).abs() < 0.01, "{got:?} vs {expected:?}");
        }
    }

    fn runs(tree: &WidgetTree<WidgetKind>, id: WidgetId, scales: &Scales) -> Vec<crate::TextRun> {
        let b = bounds(tree, id);
        text_runs(tree, id, b, b, None, &dark_theme(), scales)
    }

    #[test]
    #[allow(clippy::float_cmp, clippy::cast_precision_loss)]
    fn a_measured_checkbox_draws_its_label_past_the_box_in_text_primary() {
        let scales = test_scales();
        let theme = dark_theme();
        let mut engine = engine();
        let (mut tree, ids) = column(&scales, &["Visible", "Off"]);
        let [enabled, disabled] = ids.as_slice() else {
            unreachable!("two labels");
        };
        ok(set_checkbox_disabled(&mut tree, *disabled, true));
        layout(&mut tree, &mut engine, &scales, 1.0);
        let (side, gap) = checkbox_metrics(&scales);

        let got = runs(&tree, *enabled, &scales);
        let [run] = got.as_slice() else {
            unreachable!("one label run: {got:?}");
        };
        let b = bounds(&tree, *enabled);
        assert_eq!(run.text, "Visible");
        assert_eq!(
            run.rect,
            (
                b.x as f32 + side + gap,
                b.y as f32,
                b.width as f32 - side - gap,
                b.height as f32
            )
        );
        assert_eq!(run.align, HAlign::Start);
        let [r, g, bl] = theme.text.primary.to_srgb_f32();
        assert_eq!(run.color, [r, g, bl, 1.0]);

        let got = runs(&tree, *disabled, &scales);
        let [run] = got.as_slice() else {
            unreachable!("one label run: {got:?}");
        };
        assert_eq!(run.color, [r, g, bl, theme.state.disabled_opacity]);
    }

    #[test]
    fn an_unmeasured_or_squeezed_checkbox_draws_no_label() {
        let scales = test_scales();
        let mut engine = engine();
        let (mut tree, ids) = column(&scales, &["Visible"]);
        let [checkbox] = ids.as_slice() else {
            unreachable!("one label");
        };
        tree.compute_layout(400.0, 300.0);
        assert!(
            runs(&tree, *checkbox, &scales).is_empty(),
            "text-blind: no label"
        );
        // Unmeasured but hand-sized wide (a gallery-style cell): still no
        // label — its whole bounds are the box.
        let b = bounds(&tree, *checkbox);
        ok(tree.set_bounds(*checkbox, Rect { width: 200, ..b }));
        assert!(
            runs(&tree, *checkbox, &scales).is_empty(),
            "unmeasured cell: no label"
        );

        layout(&mut tree, &mut engine, &scales, 1.0);
        assert_eq!(runs(&tree, *checkbox, &scales).len(), 1);
        // Measured, then squeezed to exactly box + gap: no room left.
        let (side, gap) = checkbox_metrics(&scales);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let width = (side + gap) as u32;
        let b = bounds(&tree, *checkbox);
        ok(tree.set_bounds(*checkbox, Rect { width, ..b }));
        assert!(
            runs(&tree, *checkbox, &scales).is_empty(),
            "squeezed: no label"
        );
    }

    #[test]
    fn the_measured_width_never_clips_the_label_last_glyph() {
        let scales = test_scales();
        let mut engine = engine();
        let labels = ["Visible", "Checkbox", "Remember me", "WWW", "fly", "Mjj"];
        for sf in [1.0_f32, 1.25, 1.5, 2.0] {
            let (mut tree, ids) = column(&scales, &labels);
            layout(&mut tree, &mut engine, &scales, sf);
            for id in ids {
                let got = runs(&tree, id, &scales);
                let [run] = got.as_slice() else {
                    unreachable!("one label run");
                };
                let clipped = resolve_text(&mut engine, run, sf);
                let free = crate::TextRun {
                    clip: Rect {
                        x: -10_000,
                        y: -10_000,
                        width: 20_000,
                        height: 20_000,
                    },
                    ..run.clone()
                };
                let unclipped = resolve_text(&mut engine, &free, sf);
                assert!(!unclipped.is_empty());
                assert_eq!(clipped, unclipped, "{:?} at {sf}", run.text);
            }
        }
    }

    #[test]
    fn paint_after_a_measuring_layout_shapes_nothing_new() {
        let scales = test_scales();
        let theme = dark_theme();
        let mut engine = engine();
        let (mut tree, ids) = column(&scales, &["Visible", "Checkbox"]);
        layout(&mut tree, &mut engine, &scales, 2.0);
        let after_layout = engine.cached_line_count();
        assert!(after_layout >= 2);
        layout(&mut tree, &mut engine, &scales, 2.0);
        assert_eq!(
            engine.cached_line_count(),
            after_layout,
            "second pass: cache hits"
        );
        for id in ids {
            let ops = ok(paint_widget_ops_frame(
                &tree, id, None, None, &theme, &scales, 2.0,
            ));
            for op in ops {
                if let crate::PaintOp::Text(run) = op {
                    let _ = resolve_text(&mut engine, &run, 2.0);
                }
            }
        }
        assert_eq!(
            engine.cached_line_count(),
            after_layout,
            "paint reuses the lines layout shaped"
        );
    }
}
