//! Widget labels as text runs (0.132.0): which widgets carry visible
//! text, where it sits, in what style and colour, and — headless, on the
//! CPU — which glyph quads that text becomes.
//!
//! [`text_runs`] is the emission half: a pure function of a widget's own
//! state, its layout box and its visible (clipped) rect, returning the
//! [`TextRun`]s [`crate::paint_widget_ops_focused`] appends as
//! [`crate::PaintOp::Text`] after the widget's own solids and before its
//! focus ring. Every size, weight, line height, padding and colour comes
//! from a design token (invariant §7.3.10): `type.size.md`,
//! `type.weight.regular`, `type.line_height.normal`, the spacing scale,
//! and the `text.*` colour tokens whose pairs `design/check_contrast.py`
//! already gates.
//!
//! [`resolve_text`] is the layout half: it shapes a run with
//! [`aurora_text::TextEngine`] at the display's physical size, places each
//! glyph on a whole physical pixel (the fraction goes into the glyph's
//! [`aurora_text::SubpixelBin`]), and clips each glyph's quad to the run's
//! clip rect, trimming the source rectangle with it. The GPU half (the
//! glyph atlas and text pipeline) lives in `crate::render`.
//!
//! **Which widgets draw text in 0.132.0**: `Button` (label, centred),
//! `Tab` (label, centred), `TreeItem` (label, one row tall), a `Menu`'s
//! action rows, a `Dropdown`'s current value, and an open dropdown list's
//! option rows. **Not yet**: `Checkbox` (its layout box *is* the 13 px
//! square box — the label needs a layout change to sit beside it),
//! `TextField` content and caret, the command palette's query and result
//! rows, tooltip text (its payload carries no string), and dialog text.
//! There is no ellipsis: a label wider than its box is clipped.

use aurora_core::Rect;
use aurora_text::{GlyphKey, TextEngine, TextStyle, snap_glyph_origin};
use aurora_theme::{Color, Scales, Theme};

use crate::tree::{WidgetId, WidgetTree};
use crate::widgets::{WidgetKind, row_height};

/// Horizontal alignment of a run inside its content box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HAlign {
    /// Flush with the content box's left edge.
    Start,
    /// Centred in the content box (overflowing both sides equally when
    /// wider than it).
    Center,
}

/// One line of widget text, ready to be resolved into glyph quads.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    /// The text itself.
    pub text: String,
    /// Size, line height and weight, all from typography tokens.
    pub style: TextStyle,
    /// Straight, sRGB-encoded RGBA, from a `text.*` token (alpha carries
    /// the disabled opacity). Linearized by a caller for an sRGB-aware
    /// target exactly as a [`crate::Paint`]'s colour is.
    pub color: [f32; 4],
    /// The content box the line is placed in, logical px:
    /// `(x, y, width, height)`. The line is vertically centred in it.
    pub rect: (f32, f32, f32, f32),
    /// Horizontal alignment within `rect`.
    pub align: HAlign,
    /// Nothing outside this rect (logical px) is drawn — the widget's
    /// visible rect after every clipping ancestor.
    pub clip: Rect,
}

/// One glyph of a resolved run: which atlas image to sample, where to put
/// it, and which part of the image survives the clip. Every coordinate is
/// in whole *physical* pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuadGlyph {
    /// The glyph image this quad samples.
    pub key: GlyphKey,
    /// Destination `[x0, y0, x1, y1]`, physical px, exclusive max.
    pub dst: [i32; 4],
    /// Offset into the glyph's own mask of `dst`'s top-left corner (non-zero
    /// only when the clip cut the glyph's left or top edge). The source
    /// size equals `dst`'s size: glyphs are never scaled.
    pub src_offset: [u32; 2],
}

#[allow(clippy::cast_precision_loss)]
fn token_px(value: u32) -> f32 {
    value as f32
}

/// The one text style every 0.132.0 label uses: `type.size.md`,
/// `type.line_height.normal`, `type.weight.regular`.
#[must_use]
pub fn label_style(scales: &Scales) -> TextStyle {
    let t = &scales.typography;
    TextStyle {
        size_px: token_px(t.size.md),
        line_height: t.line_height.normal,
        weight: u16::try_from(t.weight.regular).unwrap_or(u16::MAX),
    }
}

fn rgba(color: Color, alpha: f32) -> [f32; 4] {
    let [r, g, b] = color.to_srgb_f32();
    [r, g, b, alpha]
}

fn opacity(disabled: bool, theme: &Theme) -> f32 {
    if disabled {
        theme.state.disabled_opacity
    } else {
        1.0
    }
}

fn rect_f32(rect: Rect) -> (f32, f32, f32, f32) {
    #[allow(clippy::cast_precision_loss)]
    (
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    )
}

/// `rect` inset horizontally by `pad` on both sides.
fn inset_x(rect: (f32, f32, f32, f32), pad: f32) -> (f32, f32, f32, f32) {
    (rect.0 + pad, rect.1, (rect.2 - 2.0 * pad).max(0.0), rect.3)
}

/// The label a menu's or an open dropdown list's option row shows, with
/// whether that entry is enabled.
fn row_label(tree: &WidgetTree<WidgetKind>, row: WidgetId) -> Option<(String, bool)> {
    let parent = tree.parent(row)?;
    match tree.payload(parent)? {
        WidgetKind::Menu(menu) => {
            let index = menu.item_ids().iter().position(|id| *id == row)?;
            let item = menu.items().get(index)?;
            Some((item.label.clone(), item.enabled))
        }
        WidgetKind::DropdownList => {
            let dropdown = tree.parent(parent)?;
            let Some(WidgetKind::Dropdown(state)) = tree.payload(dropdown) else {
                return None;
            };
            let index = state.rows().iter().position(|id| *id == row)?;
            let option = state.options().get(index)?;
            Some((option.clone(), !state.is_disabled()))
        }
        _ => None,
    }
}

/// The text runs widget `id` draws, in paint order (at most one today).
/// `bounds` is the widget's own layout box and `clip` its visible rect
/// (`WidgetTree::visible_rect`); a widget with no visible rect draws no
/// text, which the caller guarantees by not calling this.
#[must_use]
pub fn text_runs(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
    bounds: Rect,
    clip: Rect,
    theme: &Theme,
    scales: &Scales,
) -> Vec<TextRun> {
    let Some(kind) = tree.payload(id) else {
        return Vec::new();
    };
    let style = label_style(scales);
    let pad = token_px(scales.spacing.sm);
    let full = rect_f32(bounds);
    let run = |text: &str, color: [f32; 4], rect, align| TextRun {
        text: sanitize_label(text),
        style,
        color,
        rect,
        align,
        clip,
    };
    let runs = match kind {
        WidgetKind::Button(state) => vec![run(
            &state.label,
            rgba(theme.text.on_accent, opacity(state.disabled, theme)),
            full,
            HAlign::Center,
        )],
        WidgetKind::Tab(state) => {
            let color = if state.is_selected() {
                theme.text.primary
            } else {
                theme.text.secondary
            };
            vec![run(
                &state.label,
                rgba(color, opacity(state.is_disabled(), theme)),
                full,
                HAlign::Center,
            )]
        }
        WidgetKind::TreeItem(state) => {
            // One row tall, like the selection fill: a row's own box
            // grows to contain its children.
            let height = row_height(scales).min(full.3);
            let color = if state.selected {
                theme.text.on_accent
            } else {
                theme.text.primary
            };
            vec![run(
                &state.label,
                rgba(color, opacity(state.disabled, theme)),
                inset_x((full.0, full.1, full.2, height), pad),
                HAlign::Start,
            )]
        }
        WidgetKind::Dropdown(state) => state
            .selected_option()
            .map(|value| {
                run(
                    value,
                    rgba(theme.text.primary, opacity(state.is_disabled(), theme)),
                    inset_x(full, pad),
                    HAlign::Start,
                )
            })
            .into_iter()
            .collect(),
        WidgetKind::ListRow(row) => row_label(tree, id)
            .map(|(label, enabled)| {
                // A selected row paints an `accent.primary` highlight,
                // so its text is `text.on_accent` (gated at 4.5:1 against
                // it); a disabled entry uses `text.disabled`.
                let color = if !enabled || row.disabled {
                    theme.text.disabled
                } else if row.selected {
                    theme.text.on_accent
                } else {
                    theme.text.primary
                };
                run(&label, rgba(color, 1.0), inset_x(full, pad), HAlign::Start)
            })
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    runs.into_iter().filter(|r| !r.text.is_empty()).collect()
}

/// `text` with every control character (C0, DEL, C1 — tab and newline
/// included) replaced by a space. Labels are one line, shaped left to
/// right in one bundled font with no fallback: a control character has no
/// glyph to draw and would otherwise shape as a `.notdef` box.
fn sanitize_label(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// The largest physical font size [`resolve_text`] rasterizes: a glyph
/// whose em square alone exceeds the glyph atlas can never be placed, so
/// it is skipped before it costs a rasterization.
pub const MAX_GLYPH_SIZE_PHYS: f32 = 1024.0;

/// A logical coordinate on the physical grid, rounded to the pixel whose
/// centre it covers — the same rule the rasterizer applies to a solid's
/// edge, so text is clipped exactly where a clipped fill ends.
#[allow(clippy::cast_possible_truncation)]
fn to_phys(logical: f32, scale_factor: f32) -> i32 {
    let v = (logical * scale_factor).round();
    if v.is_finite() {
        v.clamp(i32::MIN as f32, i32::MAX as f32) as i32
    } else {
        0
    }
}

/// Resolves `run` into positioned, clipped glyph quads for a display at
/// `scale_factor`. Headless: needs a [`TextEngine`], no GPU.
///
/// The line box is vertically centred in `run.rect` and the font's
/// content area (ascent + descent) is centred in the line box — CSS
/// half-leading — so the baseline sits at
/// `y + (h - line_height) / 2 + (line_height - ascent - descent) / 2 + ascent`,
/// which simplifies to `y + h / 2 + (ascent - descent) / 2`. The line is
/// aligned horizontally per `run.align` and its baseline snapped to a
/// whole physical pixel. A glyph with no coverage (a space), or wholly
/// outside `run.clip`, yields no quad; so does every glyph of a run whose
/// rect or scale is non-finite, or whose physical size exceeds
/// `MAX_GLYPH_SIZE_PHYS`.
pub fn resolve_text(engine: &mut TextEngine, run: &TextRun, scale_factor: f32) -> Vec<QuadGlyph> {
    let (x, y, w, h) = run.rect;
    if ![x, y, w, h, scale_factor].iter().all(|v| v.is_finite()) || scale_factor <= 0.0 {
        return Vec::new();
    }
    let line = engine.shape(&run.text, &run.style, scale_factor);
    if line.glyphs.is_empty() || line.size_phys.is_nan() || line.size_phys > MAX_GLYPH_SIZE_PHYS {
        return Vec::new();
    }
    let start_x = match run.align {
        HAlign::Start => x,
        HAlign::Center => x + (w - line.width) / 2.0,
    };
    let baseline_logical = y + h / 2.0 + (line.ascent - line.descent) / 2.0;
    let origin_x = start_x * scale_factor;
    if !(baseline_logical * scale_factor).is_finite() || !origin_x.is_finite() {
        return Vec::new();
    }
    let baseline = to_phys(baseline_logical, scale_factor);

    let clip_x0 = to_phys(rect_f32(run.clip).0, scale_factor);
    let clip_y0 = to_phys(rect_f32(run.clip).1, scale_factor);
    let (cx, cy, cw, ch) = rect_f32(run.clip);
    let clip_x1 = to_phys(cx + cw, scale_factor);
    let clip_y1 = to_phys(cy + ch, scale_factor);

    let mut quads = Vec::with_capacity(line.glyphs.len());
    for glyph in &line.glyphs {
        let (pixel_x, bin) = snap_glyph_origin(origin_x + glyph.x_phys);
        let key = GlyphKey::new(glyph.glyph_id, line.size_phys, bin);
        let Some(mask) = engine.glyph(key) else {
            continue;
        };
        #[allow(clippy::cast_possible_truncation)]
        let y_offset = glyph.y_phys.round() as i32;
        let x0 = pixel_x.saturating_add(mask.left);
        let y0 = baseline.saturating_add(y_offset).saturating_sub(mask.top);
        let x1 = x0.saturating_add(i32::try_from(mask.width).unwrap_or(i32::MAX));
        let y1 = y0.saturating_add(i32::try_from(mask.height).unwrap_or(i32::MAX));
        let (qx0, qy0) = (x0.max(clip_x0), y0.max(clip_y0));
        let (qx1, qy1) = (x1.min(clip_x1), y1.min(clip_y1));
        if qx0 >= qx1 || qy0 >= qy1 {
            continue;
        }
        let src_offset = [
            u32::try_from(qx0.saturating_sub(x0)).unwrap_or(0),
            u32::try_from(qy0.saturating_sub(y0)).unwrap_or(0),
        ];
        quads.push(QuadGlyph {
            key,
            dst: [qx0, qy0, qx1, qy1],
            src_offset,
        });
    }
    quads
}

#[cfg(test)]
// Colours are compared bit-exactly: both sides come from the same
// `Color::to_srgb_f32` on the same token, never from accumulated arithmetic.
#[allow(clippy::float_cmp)]
mod tests {
    use super::{HAlign, TextRun, resolve_text, text_runs};
    use crate::paint::{FocusPaint, PaintOp, paint_widget_ops, paint_widget_ops_focused};
    use crate::tree::{WidgetId, WidgetTree};
    use crate::widgets::{
        MenuItem, MenuKey, WidgetKind, dropdown_state, handle_menu_key, insert_button,
        insert_container, insert_dropdown, insert_tab_bar, insert_tree_item, insert_tree_view,
        menu_state, new_tree, open_menu, row_height, set_button_disabled, set_dropdown_disabled,
        set_dropdown_open, set_tab_bar_disabled, set_tree_item_disabled, set_tree_item_selected,
        tab_bar_state, test_scales,
    };
    use aurora_core::Rect;
    use aurora_text::TextEngine;
    use aurora_theme::{Palette, Scales, Theme, ThemeSet};

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

    fn place(tree: &mut WidgetTree<WidgetKind>, id: WidgetId, x: i64, y: i64, w: u32, h: u32) {
        ok(tree.set_bounds(
            id,
            Rect {
                x,
                y,
                width: w,
                height: h,
            },
        ));
    }

    fn button(label: &str) -> (WidgetTree<WidgetKind>, WidgetId, Scales) {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        place(&mut tree, root, 0, 0, 400, 300);
        let id = ok(insert_button(&mut tree, root, &scales, label));
        place(&mut tree, id, 20, 10, 100, 30);
        (tree, id, scales)
    }

    fn texts(ops: &[PaintOp]) -> Vec<&TextRun> {
        ops.iter()
            .filter_map(|op| match op {
                PaintOp::Text(run) => Some(run),
                _ => None,
            })
            .collect()
    }

    fn rgba(color: aurora_theme::Color, a: f32) -> [f32; 4] {
        let [r, g, b] = color.to_srgb_f32();
        [r, g, b, a]
    }

    #[test]
    fn button_emits_one_text_run_after_its_solids_with_on_accent_colour() {
        let (tree, id, scales) = button("Apply");
        let theme = dark_theme();
        let ops = ok(paint_widget_ops(&tree, id, &theme, &scales, 1.0));
        let first_text = ops.iter().position(|op| matches!(op, PaintOp::Text(_)));
        let last_solid = ops.iter().rposition(|op| matches!(op, PaintOp::Solid(_)));
        assert!(first_text.is_some() && last_solid.is_some());
        assert!(
            first_text > last_solid,
            "text must paint on top of the fill"
        );
        let runs = texts(&ops);
        assert_eq!(runs.len(), 1);
        let run = runs.first().copied();
        assert!(run.is_some_and(|r| r.text == "Apply"
            && r.color == rgba(theme.text.on_accent, 1.0)
            && r.align == HAlign::Center
            && r.rect == (20.0, 10.0, 100.0, 30.0)));
    }

    #[test]
    fn disabled_button_text_alpha_is_disabled_opacity() {
        let (mut tree, id, scales) = button("Apply");
        ok(set_button_disabled(&mut tree, id, true));
        let theme = dark_theme();
        let ops = ok(paint_widget_ops(&tree, id, &theme, &scales, 1.0));
        let alpha = texts(&ops).first().map(|r| r.color[3]);
        assert_eq!(alpha, Some(theme.state.disabled_opacity));
    }

    #[test]
    fn text_style_comes_from_the_typography_tokens() {
        let (tree, id, scales) = button("Apply");
        let theme = dark_theme();
        let ops = ok(paint_widget_ops(&tree, id, &theme, &scales, 1.0));
        let style = texts(&ops).first().map(|r| r.style);
        let t = &scales.typography;
        assert!(
            style.is_some_and(|s| s.size_px.to_bits() == (t.size.md as f32).to_bits()
                && s.line_height.to_bits() == t.line_height.normal.to_bits()
                && u32::from(s.weight) == t.weight.regular)
        );
    }

    #[test]
    fn selected_tab_text_is_primary_unselected_is_secondary() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        place(&mut tree, root, 0, 0, 400, 300);
        let bar = ok(insert_tab_bar(
            &mut tree,
            root,
            &scales,
            "Panels",
            vec!["Layers".to_owned(), "History".to_owned()],
            1,
        ));
        place(&mut tree, bar, 0, 0, 200, 30);
        let tabs: Vec<WidgetId> = tree.children(bar).unwrap_or_default().to_vec();
        assert_eq!(tabs.len(), 2);
        for (i, tab) in tabs.iter().enumerate() {
            place(
                &mut tree,
                *tab,
                i64::try_from(i).unwrap_or(0) * 100,
                0,
                100,
                30,
            );
        }
        let theme = dark_theme();
        let colors: Vec<[f32; 4]> = tabs
            .iter()
            .flat_map(|tab| {
                texts(&ok(paint_widget_ops(&tree, *tab, &theme, &scales, 1.0)))
                    .into_iter()
                    .map(|r| r.color)
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(
            colors,
            vec![
                rgba(theme.text.secondary, 1.0),
                rgba(theme.text.primary, 1.0)
            ]
        );
    }

    #[test]
    fn tree_row_text_rect_is_clamped_to_one_row_and_selected_is_on_accent() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        place(&mut tree, root, 0, 0, 400, 300);
        let view = ok(insert_tree_view(&mut tree, root, Some("Layers")));
        place(&mut tree, view, 0, 0, 200, 200);
        let row = ok(insert_tree_item(&mut tree, view, &scales, "Group", true));
        place(&mut tree, row, 0, 0, 200, 90);
        let theme = dark_theme();
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 90,
        };
        let runs = text_runs(&tree, row, bounds, bounds, &theme, &scales);
        let run = runs.first();
        let pad = scales.spacing.sm as f32;
        assert!(
            run.is_some_and(|r| (r.rect.3 - row_height(&scales)).abs() < 1e-6
                && (r.rect.0 - pad).abs() < 1e-6
                && r.color == rgba(theme.text.primary, 1.0))
        );
        ok(set_tree_item_selected(&mut tree, row, true));
        let runs = text_runs(&tree, row, bounds, bounds, &theme, &scales);
        assert_eq!(
            runs.first().map(|r| r.color),
            Some(rgba(theme.text.on_accent, 1.0))
        );
    }

    fn clipped_button(offset: i64) -> (WidgetTree<WidgetKind>, WidgetId, Scales) {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        place(&mut tree, root, 0, 0, 400, 300);
        let clip = ok(insert_container(
            &mut tree,
            root,
            taffy::Style {
                overflow: taffy::Point {
                    x: taffy::Overflow::Hidden,
                    y: taffy::Overflow::Hidden,
                },
                ..Default::default()
            },
        ));
        place(&mut tree, clip, 0, 0, 100, 100);
        let id = ok(insert_button(&mut tree, clip, &scales, "Clipped label"));
        place(&mut tree, id, offset, 40, 100, 24);
        (tree, id, scales)
    }

    #[test]
    fn a_fully_clipped_widget_emits_no_text() {
        let (tree, id, scales) = clipped_button(150);
        let ops = ok(paint_widget_ops(&tree, id, &dark_theme(), &scales, 1.0));
        assert!(ops.is_empty(), "{ops:?}");
    }

    #[test]
    fn text_clip_is_the_visible_rect() {
        let (tree, id, scales) = clipped_button(40);
        let ops = ok(paint_widget_ops(&tree, id, &dark_theme(), &scales, 1.0));
        let clip = texts(&ops).first().map(|r| r.clip);
        assert_eq!(
            clip,
            Some(Rect {
                x: 40,
                y: 40,
                width: 60,
                height: 24
            })
        );
    }

    #[test]
    fn focus_ring_ops_come_after_text() {
        let (mut tree, id, scales) = button("Focus me");
        let theme = dark_theme();
        let mut focus = crate::input::FocusManager::new();
        ok(focus.focus_with(&mut tree, id, crate::input::FocusOrigin::Keyboard));
        let Some(paint) = FocusPaint::resolve(&tree, &focus) else {
            unreachable!("a keyboard-focused button has a ring");
        };
        let ops = ok(paint_widget_ops_focused(
            &tree,
            id,
            Some(paint),
            &theme,
            &scales,
            1.0,
        ));
        let text = ops.iter().position(|op| matches!(op, PaintOp::Text(_)));
        assert!(text.is_some());
        assert_eq!(
            ops.len().checked_sub(3),
            text,
            "text, then the two ring ops"
        );
    }

    #[test]
    fn resolve_text_centres_the_run_in_the_button() {
        let (tree, id, scales) = button("OK");
        let ops = ok(paint_widget_ops(&tree, id, &dark_theme(), &scales, 1.0));
        let Some(run) = texts(&ops).first().copied().cloned() else {
            unreachable!("a button has a label");
        };
        let mut engine = engine();
        let quads = resolve_text(&mut engine, &run, 1.0);
        assert_eq!(quads.len(), 2);
        let left = quads.iter().map(|q| q.dst[0]).min().unwrap_or_default();
        let right = quads.iter().map(|q| q.dst[2]).max().unwrap_or_default();
        let top = quads.iter().map(|q| q.dst[1]).min().unwrap_or_default();
        let bottom = quads.iter().map(|q| q.dst[3]).max().unwrap_or_default();
        // Button spans x 20..120, y 10..40: ink is centred to within the
        // side bearings and the snap, and sits inside the box.
        let ink_centre_x = f64::from(left + right) / 2.0;
        assert!(
            (ink_centre_x - 70.0).abs() <= 2.0,
            "ink centre {ink_centre_x}"
        );
        // Half-leading centring: the baseline is at
        // `y + h/2 + (ascent - descent)/2`, snapped; capitals ("OK") sit
        // on it and their ink centre lands on the box's centre, 25.
        let line = engine.shape("OK", &run.style, 1.0);
        let target = (10.0 + 15.0 + (line.ascent - line.descent) / 2.0).round();
        #[allow(clippy::cast_possible_truncation)]
        let target = target as i32;
        assert!(
            (bottom - target).abs() <= 1,
            "ink bottom {bottom} vs half-leading baseline {target}"
        );
        let ink_centre_y = f64::from(top + bottom) / 2.0;
        assert!(
            (ink_centre_y - 25.0).abs() <= 1.0,
            "ink centre y {ink_centre_y}"
        );
    }

    #[test]
    fn resolve_text_draws_nothing_for_a_non_finite_rect_or_scale() {
        let mut engine = engine();
        let clip = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 60,
        };
        for rect in [
            (f32::NAN, 10.0, 100.0, 20.0),
            (10.0, f32::INFINITY, 100.0, 20.0),
            (10.0, 10.0, f32::NAN, 20.0),
            (10.0, 10.0, 100.0, f32::NEG_INFINITY),
            (f32::MAX, 10.0, 100.0, 20.0),
        ] {
            let run = TextRun {
                rect,
                ..a_run(clip)
            };
            assert!(resolve_text(&mut engine, &run, 1.0).is_empty(), "{rect:?}");
        }
        for scale in [f32::NAN, 0.0, -1.0, f32::INFINITY] {
            assert!(
                resolve_text(&mut engine, &a_run(clip), scale).is_empty(),
                "{scale}"
            );
        }
        // A size past the atlas is skipped before rasterizing.
        assert!(resolve_text(&mut engine, &a_run(clip), 200.0).is_empty());
        assert!(!resolve_text(&mut engine, &a_run(clip), 1.0).is_empty());
    }

    #[test]
    fn control_characters_in_a_label_become_spaces() {
        let (tree, id, scales) = button("A\tB\nC\u{7f}D\u{85}E");
        let ops = ok(paint_widget_ops(&tree, id, &dark_theme(), &scales, 1.0));
        let text = texts(&ops).first().map(|r| r.text.clone());
        assert_eq!(text.as_deref(), Some("A B C D E"));
    }

    fn row_runs(
        tree: &WidgetTree<WidgetKind>,
        rows: &[WidgetId],
        theme: &Theme,
        scales: &Scales,
    ) -> Vec<(String, [f32; 4])> {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 24,
        };
        rows.iter()
            .flat_map(|row| text_runs(tree, *row, bounds, bounds, theme, scales))
            .map(|r| (r.text, r.color))
            .collect()
    }

    #[test]
    fn menu_rows_label_in_order_skip_separators_and_colour_by_state() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        place(&mut tree, root, 0, 0, 400, 300);
        let mut save = MenuItem::action("Save");
        save.enabled = false;
        let items = vec![
            MenuItem::action("Open"),
            MenuItem::separator(),
            save,
            MenuItem::action("Quit"),
        ];
        let menu = ok(open_menu(
            &mut tree,
            root,
            &scales,
            "File",
            (0.0, 0.0),
            160.0,
            items,
        ));
        // Highlight the last enabled action, "Quit" (past the disabled one).
        let _ = ok(handle_menu_key(&mut tree, menu, MenuKey::End));
        let rows = ok(menu_state(&tree, menu)).item_ids().to_vec();
        assert_eq!(rows.len(), 4, "the separator is a row too");
        assert_eq!(ok(menu_state(&tree, menu)).highlighted(), Some(3));
        let theme = dark_theme();
        assert_ne!(theme.text.primary, theme.text.on_accent);
        let runs = row_runs(&tree, &rows, &theme, &scales);
        assert_eq!(
            runs,
            vec![
                ("Open".to_owned(), rgba(theme.text.primary, 1.0)),
                ("Save".to_owned(), rgba(theme.text.disabled, 1.0)),
                ("Quit".to_owned(), rgba(theme.text.on_accent, 1.0)),
            ],
            "in order, the separator draws no text, disabled and highlighted rows recoloured"
        );
        // The menu item, not only the row's mirrored flag, decides: a row
        // payload desynced to "enabled" still draws a disabled item dim.
        if let Some(WidgetKind::ListRow(row)) = rows.get(2).and_then(|r| tree.payload_mut(*r)) {
            row.disabled = false;
        } else {
            unreachable!("Save's row is a ListRow");
        }
        let runs = row_runs(&tree, &rows, &theme, &scales);
        assert_eq!(
            runs.get(1).map(|(_, c)| *c),
            Some(rgba(theme.text.disabled, 1.0))
        );
    }

    #[test]
    fn an_open_dropdown_list_labels_its_options_and_highlights_on_accent() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        place(&mut tree, root, 0, 0, 400, 300);
        let options = vec!["Small".to_owned(), "Medium".to_owned(), "Large".to_owned()];
        let dropdown = ok(insert_dropdown(
            &mut tree,
            root,
            &scales,
            "Size",
            options,
            Some(1),
        ));
        place(&mut tree, dropdown, 0, 0, 160, 24);
        let _ = ok(set_dropdown_open(&mut tree, dropdown, true));
        let state = ok(dropdown_state(&tree, dropdown));
        assert!(state.is_open());
        let rows = state.rows().to_vec();
        let highlighted = state.highlighted();
        assert_eq!(highlighted, Some(1), "opening highlights the selection");
        let theme = dark_theme();
        let runs = row_runs(&tree, &rows, &theme, &scales);
        assert_eq!(
            runs,
            vec![
                ("Small".to_owned(), rgba(theme.text.primary, 1.0)),
                ("Medium".to_owned(), rgba(theme.text.on_accent, 1.0)),
                ("Large".to_owned(), rgba(theme.text.primary, 1.0)),
            ]
        );
        // The closed dropdown's own value is drawn too, in text.primary.
        let value = texts(&ok(paint_widget_ops(&tree, dropdown, &theme, &scales, 1.0)))
            .first()
            .map(|r| (r.text.clone(), r.color));
        assert_eq!(
            value,
            Some(("Medium".to_owned(), rgba(theme.text.primary, 1.0)))
        );
        // Disabling the dropdown closes its list, so no option row can
        // draw enabled text for a disabled dropdown (`row_label`'s own
        // `!is_disabled()` is a second line of defence for a payload
        // written back directly, unreachable through this API).
        ok(set_dropdown_disabled(&mut tree, dropdown, true));
        assert!(ok(dropdown_state(&tree, dropdown)).rows().is_empty());
        assert!(row_runs(&tree, &rows, &theme, &scales).is_empty());
    }

    #[test]
    // One table across five widget kinds; splitting it would only scatter
    // the table.
    #[allow(clippy::too_many_lines, clippy::type_complexity)]
    fn every_labelled_widget_dims_its_text_when_disabled() {
        let theme = dark_theme();
        let dim = theme.state.disabled_opacity;
        assert!(dim < 1.0);
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 24,
        };
        let scales = test_scales();
        let run_colour = |tree: &WidgetTree<WidgetKind>, id| {
            text_runs(tree, id, bounds, bounds, &theme, &scales)
                .first()
                .map(|r| r.color)
        };
        // (widget, enabled colour, disabled colour)
        let mut cases: Vec<(&str, Option<[f32; 4]>, Option<[f32; 4]>)> = Vec::new();
        for disabled in [false, true] {
            let (mut tree, root) = new_tree(taffy::Style::default());
            place(&mut tree, root, 0, 0, 400, 300);
            let button = ok(insert_button(&mut tree, root, &scales, "Apply"));
            ok(set_button_disabled(&mut tree, button, disabled));
            let bar = ok(insert_tab_bar(
                &mut tree,
                root,
                &scales,
                "Panels",
                vec!["Layers".to_owned()],
                0,
            ));
            ok(set_tab_bar_disabled(&mut tree, bar, disabled));
            let tab = ok(tab_bar_state(&tree, bar)).tabs().first().copied();
            let view = ok(insert_tree_view(&mut tree, root, Some("Layers")));
            let item = ok(insert_tree_item(&mut tree, view, &scales, "Group", false));
            ok(set_tree_item_disabled(&mut tree, item, disabled));
            let dropdown = ok(insert_dropdown(
                &mut tree,
                root,
                &scales,
                "Size",
                vec!["Small".to_owned()],
                Some(0),
            ));
            ok(set_dropdown_disabled(&mut tree, dropdown, disabled));
            let mut entry = MenuItem::action("Open");
            entry.enabled = !disabled;
            let menu = ok(open_menu(
                &mut tree,
                root,
                &scales,
                "File",
                (0.0, 0.0),
                160.0,
                vec![MenuItem::action("Quit"), entry],
            ));
            let menu_row = ok(menu_state(&tree, menu)).item_ids().get(1).copied();
            let colours = [
                ("button", run_colour(&tree, button)),
                ("tab", tab.and_then(|t| run_colour(&tree, t))),
                ("tree item", run_colour(&tree, item)),
                ("dropdown value", run_colour(&tree, dropdown)),
                ("menu row", menu_row.and_then(|r| run_colour(&tree, r))),
            ];
            for (i, (name, colour)) in colours.into_iter().enumerate() {
                if disabled {
                    if let Some(case) = cases.get_mut(i) {
                        case.2 = colour;
                    }
                } else {
                    cases.push((name, colour, None));
                }
            }
        }
        let expected = [
            (
                "button",
                rgba(theme.text.on_accent, 1.0),
                rgba(theme.text.on_accent, dim),
            ),
            (
                "tab",
                rgba(theme.text.primary, 1.0),
                rgba(theme.text.primary, dim),
            ),
            (
                "tree item",
                rgba(theme.text.primary, 1.0),
                rgba(theme.text.primary, dim),
            ),
            (
                "dropdown value",
                rgba(theme.text.primary, 1.0),
                rgba(theme.text.primary, dim),
            ),
            (
                "menu row",
                rgba(theme.text.primary, 1.0),
                rgba(theme.text.disabled, 1.0),
            ),
        ];
        assert_eq!(cases.len(), expected.len());
        for ((name, enabled, disabled), (ename, e_on, e_off)) in cases.iter().zip(expected) {
            assert_eq!(*name, ename);
            assert_eq!(*enabled, Some(e_on), "{name} enabled");
            assert_eq!(*disabled, Some(e_off), "{name} disabled");
        }
    }

    fn a_run(clip: Rect) -> TextRun {
        TextRun {
            text: "Hello".to_owned(),
            style: super::label_style(&test_scales()),
            color: [1.0; 4],
            rect: (10.0, 10.0, 100.0, 20.0),
            align: HAlign::Start,
            clip,
        }
    }

    #[test]
    fn resolve_text_drops_glyphs_outside_clip_and_trims_uvs() {
        let mut engine = engine();
        let wide = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        };
        let full = resolve_text(&mut engine, &a_run(wide), 1.0);
        assert_eq!(full.len(), 5);
        assert!(full.iter().all(|q| q.src_offset == [0, 0]));
        // Clip through the middle of the first glyph, "H", and above
        // the baseline so every glyph is cut at the bottom.
        let Some(h) = full.first().copied() else {
            unreachable!()
        };
        let cut_x = i32::midpoint(h.dst[0], h.dst[2]);
        let cut_bottom = i32::midpoint(h.dst[1], h.dst[3]);
        let clip = Rect {
            x: i64::from(cut_x),
            y: 0,
            width: 10,
            height: u32::try_from(cut_bottom).unwrap_or(0),
        };
        let clipped = resolve_text(&mut engine, &a_run(clip), 1.0);
        assert!(!clipped.is_empty() && clipped.len() < full.len());
        for q in &clipped {
            assert!(q.dst[0] >= cut_x && q.dst[2] <= cut_x + 10 && q.dst[3] <= cut_bottom);
        }
        let first = clipped.first().copied();
        assert_eq!(first.map(|q| q.key), Some(h.key));
        assert_eq!(
            first.map(|q| q.src_offset),
            Some([u32::try_from(cut_x - h.dst[0]).unwrap_or(0), 0])
        );
    }

    #[test]
    fn quads_land_on_whole_physical_pixels_at_scale_2() {
        let mut engine = engine();
        let clip = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        };
        let mut run = a_run(clip);
        run.rect = (10.3, 10.7, 100.0, 20.0);
        let at_one = resolve_text(&mut engine, &run, 1.0);
        let at_two = resolve_text(&mut engine, &run, 2.0);
        assert_eq!(at_one.len(), at_two.len());
        // Physical glyphs at scale 2 are rasterized twice as large.
        let height = |q: &super::QuadGlyph| q.dst[3] - q.dst[1];
        let (h1, h2) = (
            at_one.first().map(height).unwrap_or_default(),
            at_two.first().map(height).unwrap_or_default(),
        );
        assert!(h2 >= 2 * h1 - 2 && h2 <= 2 * h1 + 2, "{h1} vs {h2}");
        // Baselines of a same-line run share one whole-pixel row: every
        // glyph of "Hello" (no descenders) ends on the same pixel.
        let bottoms: Vec<i32> = at_two.iter().map(|q| q.dst[3]).collect();
        assert!(
            bottoms.windows(2).all(|w| match (w.first(), w.get(1)) {
                (Some(a), Some(b)) => (a - b).abs() <= 1,
                _ => true,
            }),
            "{bottoms:?}"
        );
        // x starts carry the fractional pen position in the bin, not the
        // quad: 10.3 logical * 2 = 20.6 physical rounds to bin 2.
        assert_eq!(
            at_two.first().map(|q| q.key.bin),
            Some(aurora_text::snap_glyph_origin(20.6).1)
        );
    }
}
