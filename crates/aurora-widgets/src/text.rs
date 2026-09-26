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
//! **Which widgets draw text**: since 0.132.0 `Button` (label, centred),
//! `Tab` (label, centred), `TreeItem` (label, one row tall), a `Menu`'s
//! action rows, a `Dropdown`'s current value, and an open dropdown list's
//! option rows. Since 0.133.0 also a `TextField`'s content — with its
//! caret while focused ([`CARET_WIDTH`], no blink), its selection
//! (`accent.primary` highlight, selected glyphs redrawn in
//! `text.on_accent`) and its IME preedit spliced in at the cursor and
//! underlined ([`FieldDecor`], [`resolve_run`]), horizontally scrolled
//! to keep the caret visible ([`field_scroll`], caret-pinned, not
//! sticky) — the command palette's query strip (query plus caret) and
//! result rows, a tooltip's text (its accessibility label, the one copy
//! `Tooltip::set_text` keeps current), and a dialog's message (one
//! line). **Not yet**: `Checkbox` (its layout box *is* the 13 px square
//! box — the label needs a measure-func layout pass to sit beside it), a
//! dialog's title (no layout slot), and a text field's placeholder
//! (`TextFieldState` has none). There is no ellipsis and no wrapping: a
//! line wider than its box is clipped.

use std::ops::Range;

use aurora_core::Rect;
use aurora_text::{GlyphKey, ShapedLine, TextEngine, TextStyle, snap_glyph_origin};
use aurora_theme::{Color, Scales, Theme};

use crate::tree::{WidgetId, WidgetTree};
use crate::widgets::{
    TextFieldState, UnderlineStyle, WidgetKind, composition_segments, floor_char_boundary,
    row_height,
};

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
    /// An editable line's caret, selection and IME underlines (0.133.0):
    /// `Some` only for a text field's content and the command palette's
    /// query. A run with decor is placed flush left in `rect` (whatever
    /// `align` says) and scrolled horizontally so its caret stays inside
    /// `rect` ([`field_scroll`]).
    pub field: Option<FieldDecor>,
}

impl TextRun {
    /// Applies `f` to every colour this run carries — its text colour and
    /// every [`FieldDecor`] colour — e.g. to linearize them all for an
    /// sRGB-aware target in one place.
    #[must_use]
    pub fn map_colors(mut self, f: impl Fn([f32; 4]) -> [f32; 4]) -> Self {
        self.color = f(self.color);
        if let Some(field) = self.field.as_mut() {
            field.caret_color = f(field.caret_color);
            field.selection_fill = f(field.selection_fill);
            field.selected_text = f(field.selected_text);
            field.underline_color = f(field.underline_color);
        }
        self
    }
}

/// The caret's width in logical pixels (rounded to at least one physical
/// pixel). **Not a token**: `design/tokens/scales.toml` has no
/// stroke-weight scale — the same gap [`crate::paint::FOCUS_RING_WIDTH`]
/// records. Its width, colour (`text.primary`, provisional) and the
/// absence of a blink are flagged to the design owner (Cahya, PRD
/// FR-027 *Ownership*) rather than invented here.
pub const CARET_WIDTH: f32 = 1.0;

/// An editable line's decorations. Every byte offset indexes
/// [`TextRun::text`] (for a field mid-composition that is the *display*
/// text — content with the preedit spliced in at the cursor).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecor {
    /// The byte the horizontal scroll keeps visible — the cursor, whether
    /// or not a caret is drawn there (so a field does not jump when it
    /// loses focus).
    pub scroll_anchor: usize,
    /// Where the caret is drawn, or `None` for no caret (unfocused or
    /// disabled). Not a grapheme boundary: drawn at the previous one.
    pub caret: Option<usize>,
    /// The caret's colour (`text.primary`, provisional).
    pub caret_color: [f32; 4],
    /// The selected byte range, if any and non-empty.
    pub selection: Option<Range<usize>>,
    /// The selection highlight (`accent.primary`).
    pub selection_fill: [f32; 4],
    /// Selected glyphs are redrawn over the highlight in this colour
    /// (`text.on_accent`, gated at 4.5:1 against `accent.primary`).
    pub selected_text: [f32; 4],
    /// IME preedit underlines ([`composition_segments`], shifted into the
    /// display text).
    pub underlines: Vec<(Range<usize>, UnderlineStyle)>,
    /// The underlines' colour (`text.primary`).
    pub underline_color: [f32; 4],
}

/// One drawable piece of a resolved run, in paint order.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// Glyph quads, drawn in one colour (not yet target-converted).
    Glyphs(Vec<QuadGlyph>, [f32; 4]),
    /// A solid rectangle `[x0, y0, x1, y1]` in whole physical pixels
    /// (exclusive max) — a selection highlight, underline or caret.
    Rect([i32; 4], [f32; 4]),
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

/// The label a menu's, an open dropdown list's or a command palette's
/// option row shows, with whether that entry is enabled.
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
        WidgetKind::Container => {
            // A command palette's result row: body container, then root.
            let Some(WidgetKind::CommandPalette(state)) = tree.payload(tree.parent(parent)?) else {
                return None;
            };
            state.row_title(row).map(|title| (title.to_owned(), true))
        }
        _ => None,
    }
}

/// The intersection of two rects, or `None` when they do not overlap.
fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + i64::from(a.width)).min(b.x + i64::from(b.width));
    let y1 = (a.y + i64::from(a.height)).min(b.y + i64::from(b.height));
    Some(Rect {
        x: x0,
        y: y0,
        width: u32::try_from(x1.checked_sub(x0)?).ok().filter(|w| *w > 0)?,
        height: u32::try_from(y1.checked_sub(y0)?).ok().filter(|h| *h > 0)?,
    })
}

/// `rect` inset horizontally by `pad` logical px on both sides, as an
/// integer [`Rect`] (tokens are whole pixels).
fn inset_rect(rect: Rect, pad: u32) -> Option<Rect> {
    let width = rect.width.checked_sub(pad.checked_mul(2)?)?;
    (width > 0).then_some(Rect {
        x: rect.x + i64::from(pad),
        width,
        ..rect
    })
}

/// The colours every [`FieldDecor`] takes from the theme.
fn decor(theme: &Theme, scroll_anchor: usize) -> FieldDecor {
    FieldDecor {
        scroll_anchor,
        caret: None,
        caret_color: rgba(theme.text.primary, 1.0),
        selection: None,
        selection_fill: rgba(theme.accent.primary, 1.0),
        selected_text: rgba(theme.text.on_accent, 1.0),
        underlines: Vec::new(),
        underline_color: rgba(theme.text.primary, 1.0),
    }
}

/// A text field's display text and decorations: the content with any IME
/// preedit spliced in at the cursor (underlined per
/// [`composition_segments`], caret at the preedit's end), otherwise the
/// content with its selection. The caret is drawn only when `focused`
/// and not disabled.
fn field_text(state: &TextFieldState, focused: bool, theme: &Theme) -> (String, FieldDecor) {
    // A cursor (or selection end) that is not a char boundary falls back
    // to the previous boundary, as `caret_at` does for a grapheme; one
    // past the end falls back to the end.
    let cursor = floor_char_boundary(&state.content, state.cursor);
    let show_caret = focused && !state.disabled;
    if let Some(composition) = state.composition.as_ref().filter(|c| !c.text.is_empty()) {
        let mut text = String::with_capacity(state.content.len() + composition.text.len());
        text.push_str(state.content.get(..cursor).unwrap_or_default());
        text.push_str(&composition.text);
        text.push_str(state.content.get(cursor..).unwrap_or_default());
        let end = cursor + composition.text.len();
        let mut decor = decor(theme, end);
        decor.caret = show_caret.then_some(end);
        decor.underlines = composition_segments(composition)
            .into_iter()
            .map(|(range, style)| (range.start + cursor..range.end + cursor, style))
            .collect();
        return (sanitize_field(&text), decor);
    }
    let mut decor = decor(theme, cursor);
    decor.caret = show_caret.then_some(cursor);
    decor.selection = state
        .selection_range()
        .map(|range| {
            floor_char_boundary(&state.content, range.start)
                ..floor_char_boundary(&state.content, range.end)
        })
        .filter(|range| range.start < range.end);
    (sanitize_field(&state.content), decor)
}

/// The text runs widget `id` draws, in paint order (at most one today).
/// `bounds` is the widget's own layout box and `clip` its visible rect
/// (`WidgetTree::visible_rect`); a widget with no visible rect draws no
/// text, which the caller guarantees by not calling this. `focused` is
/// the widget holding keyboard focus (any origin, pointer included): a
/// text field draws its caret only when it is `id`, the command
/// palette's query strip only when it is the palette or inside it
/// (`WidgetTree::is_within`) -- a focused result row keeps the caret.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn text_runs(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
    bounds: Rect,
    clip: Rect,
    focused: Option<WidgetId>,
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
        field: None,
    };
    // An editable line: flush left in `bounds` inset by `spacing.sm`,
    // clipped to that inset box so scrolled-away text never reaches the
    // padding. `outline` further insets the clip (not the text box, so
    // the baseline does not move) vertically by a control outline's
    // width: a text field's caret, selection and underlines then never
    // paint over its own border rows or rounded corners.
    let field_run = |text: String, color: [f32; 4], decor: FieldDecor, outline: u32| {
        let inner = inset_rect(bounds, scales.spacing.sm)?;
        let clip_box = Rect {
            y: inner.y + i64::from(outline),
            height: inner.height.checked_sub(outline.checked_mul(2)?)?,
            ..inner
        };
        Some(TextRun {
            text,
            style,
            color,
            rect: rect_f32(inner),
            align: HAlign::Start,
            clip: intersect(clip, clip_box)?,
            field: Some(decor),
        })
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
        WidgetKind::TextField(state) => {
            let (text, decor) = field_text(state, focused == Some(id), theme);
            let color = rgba(theme.text.primary, opacity(state.disabled, theme));
            // The outline is `CONTROL_BORDER_WIDTH` logical px, centred
            // on the edge; a whole logical px covers its inner half at
            // every scale factor.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let outline = crate::paint::CONTROL_BORDER_WIDTH.ceil() as u32;
            field_run(text, color, decor, outline).into_iter().collect()
        }
        // The text lives in the tooltip's own accessibility label (the
        // one copy `Tooltip::set_text` keeps current), drawn in the
        // `type.size.xs` line the tooltip's layout is sized for, inset
        // by its own `spacing.xs` horizontal padding.
        WidgetKind::Tooltip => tree
            .accessibility(id)
            .and_then(|node| node.label())
            .map(|label| {
                let small = TextStyle {
                    size_px: token_px(scales.typography.size.xs),
                    ..style
                };
                TextRun {
                    style: small,
                    ..run(
                        label,
                        rgba(theme.text.primary, 1.0),
                        inset_x(full, token_px(scales.spacing.xs)),
                        HAlign::Start,
                    )
                }
            })
            .into_iter()
            .collect(),
        WidgetKind::Container => container_text(tree, id, focused, theme)
            .map(|(text, decor)| match decor {
                Some(decor) => {
                    // The query strip has no outline of its own.
                    field_run(
                        sanitize_field(&text),
                        rgba(theme.text.primary, 1.0),
                        decor,
                        0,
                    )
                }
                None => Some(run(
                    &text,
                    rgba(theme.text.primary, 1.0),
                    full,
                    HAlign::Start,
                )),
            })
            .into_iter()
            .flatten()
            .collect(),
        _ => Vec::new(),
    };
    runs.into_iter()
        .filter(|r| {
            !r.text.is_empty() || r.field.as_ref().is_some_and(|field| field.caret.is_some())
        })
        .collect()
}

/// The text a plain container draws: a dialog's message (its
/// `Role::Label` child's label, one line, no decor), or a command
/// palette's query strip (the query, with a caret at its end while the
/// palette holds focus).
fn container_text(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
    focused: Option<WidgetId>,
    theme: &Theme,
) -> Option<(String, Option<FieldDecor>)> {
    let parent = tree.parent(id)?;
    match tree.payload(parent)? {
        WidgetKind::Dialog => {
            let node = tree.accessibility(id)?;
            (node.role() == accesskit::Role::Label)
                .then(|| node.label().map(str::to_owned))
                .flatten()
                .map(|label| (label, None))
        }
        WidgetKind::Container => {
            let root = tree.parent(parent)?;
            let WidgetKind::CommandPalette(state) = tree.payload(root)? else {
                return None;
            };
            (state.query_strip() == id).then(|| {
                let query = state.query().to_owned();
                let mut decor = decor(theme, query.len());
                // Focus anywhere in the palette (itself, or one of its
                // rows) is focus on the query.
                decor.caret = focused
                    .is_some_and(|f| tree.is_within(root, f))
                    .then_some(query.len());
                (query, Some(decor))
            })
        }
        _ => None,
    }
}

/// [`sanitize_label`] for an editable line, **byte-length preserving**:
/// each control character becomes as many spaces as it has UTF-8 bytes,
/// so every cursor, selection and preedit byte offset still lands on the
/// same character.
fn sanitize_field(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let n = if c.is_control() { c.len_utf8() } else { 0 };
            std::iter::repeat_n(' ', n).chain((n == 0).then_some(c))
        })
        .collect()
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
    let Some(placed) = place(engine, run, scale_factor, 0.0) else {
        return Vec::new();
    };
    glyph_quads(engine, &placed, clip_phys(run.clip, scale_factor))
}

/// A shaped line positioned for drawing.
struct Placed {
    line: std::sync::Arc<ShapedLine>,
    /// The pen origin's x, logical px (after alignment and scroll).
    origin_x: f32,
    /// The baseline, logical px (unsnapped).
    baseline_logical: f32,
    /// The baseline, whole physical px.
    baseline: i32,
    scale_factor: f32,
}

/// Shapes and positions `run`, its pen origin moved left by `scroll`
/// logical px. `None` for a non-finite rect or scale, or a size past
/// [`MAX_GLYPH_SIZE_PHYS`].
fn place(engine: &mut TextEngine, run: &TextRun, scale_factor: f32, scroll: f32) -> Option<Placed> {
    let (x, y, w, h) = run.rect;
    if ![x, y, w, h, scale_factor, scroll]
        .iter()
        .all(|v| v.is_finite())
        || scale_factor <= 0.0
    {
        return None;
    }
    let line = engine.shape(&run.text, &run.style, scale_factor);
    if line.size_phys.is_nan() || line.size_phys > MAX_GLYPH_SIZE_PHYS {
        return None;
    }
    let start_x = match run.align {
        HAlign::Start => x,
        HAlign::Center => x + (w - line.width) / 2.0,
    } - scroll;
    let baseline_logical = y + h / 2.0 + (line.ascent - line.descent) / 2.0;
    if !(baseline_logical * scale_factor).is_finite() || !(start_x * scale_factor).is_finite() {
        return None;
    }
    Some(Placed {
        baseline: to_phys(baseline_logical, scale_factor),
        line,
        origin_x: start_x,
        baseline_logical,
        scale_factor,
    })
}

/// `clip` (logical) on the physical grid, `[x0, y0, x1, y1]`.
fn clip_phys(clip: Rect, scale_factor: f32) -> [i32; 4] {
    let (cx, cy, cw, ch) = rect_f32(clip);
    [
        to_phys(cx, scale_factor),
        to_phys(cy, scale_factor),
        to_phys(cx + cw, scale_factor),
        to_phys(cy + ch, scale_factor),
    ]
}

/// Every inked glyph of `placed`, clipped to `clip` (physical px).
fn glyph_quads(engine: &mut TextEngine, placed: &Placed, clip: [i32; 4]) -> Vec<QuadGlyph> {
    let [clip_x0, clip_y0, clip_x1, clip_y1] = clip;
    let origin_x = placed.origin_x * placed.scale_factor;
    let line = &placed.line;
    // A glyph's ink lies within two ems of its pen position (bearings and
    // advances of the bundled font are well under one em), so a glyph
    // whose pen is further than that outside `clip` is skipped before
    // its atlas lookup: a long, scrolled field then costs one float
    // comparison per off-screen glyph, not a hash lookup.
    #[allow(clippy::cast_possible_truncation)]
    let margin = (line.size_phys * 2.0).ceil() as i32;
    let mut quads = Vec::new();
    for glyph in &line.glyphs {
        let (pixel_x, bin) = snap_glyph_origin(origin_x + glyph.x_phys);
        if pixel_x.saturating_add(margin) < clip_x0 || pixel_x.saturating_sub(margin) > clip_x1 {
            continue;
        }
        let key = GlyphKey::new(glyph.glyph_id, line.size_phys, bin);
        let Some(mask) = engine.glyph(key) else {
            continue;
        };
        #[allow(clippy::cast_possible_truncation)]
        let y_offset = glyph.y_phys.round() as i32;
        let x0 = pixel_x.saturating_add(mask.left);
        let y0 = placed
            .baseline
            .saturating_add(y_offset)
            .saturating_sub(mask.top);
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

/// The logical x of a caret before `byte` in `line`; a byte that is not
/// a grapheme boundary falls back to the previous boundary.
fn caret_at(line: &ShapedLine, byte: usize) -> f32 {
    line.caret_x(byte).unwrap_or_else(|| {
        let index = line.carets.partition_point(|(b, _)| *b <= byte);
        index
            .checked_sub(1)
            .and_then(|i| line.carets.get(i))
            .map_or(0.0, |(_, x)| *x)
    })
}

/// The horizontal scroll (logical px, `>= 0`) that keeps a caret at
/// `caret_x` inside an `inner_width`-wide box showing a `line_width`-wide
/// line: none while the line *and a caret after it* fit
/// (`line_width + CARET_WIDTH <= inner_width` -- a line that fits but
/// leaves less than a caret's width would clip an end-of-line caret);
/// otherwise the least scroll putting the
/// caret's right edge ([`CARET_WIDTH`]) at the box's right edge, clamped
/// to `[0, line_width + CARET_WIDTH - inner_width]`. **Caret-pinned, not
/// sticky**: recomputed from the caret every frame, with no memory of the
/// previous scroll (a real editor keeps its scroll until the caret
/// leaves the box; this one shows the caret at the right edge whenever
/// the line overflows and the caret is past the first box-width).
#[must_use]
pub fn field_scroll(line_width: f32, caret_x: f32, inner_width: f32) -> f32 {
    if ![line_width, caret_x, inner_width]
        .iter()
        .all(|v| v.is_finite())
        || line_width + CARET_WIDTH <= inner_width
    {
        return 0.0;
    }
    let upper = (line_width + CARET_WIDTH - inner_width).max(0.0);
    (caret_x + CARET_WIDTH - inner_width).max(0.0).min(upper)
}

/// `[x0, y0, x1, y1] ∩ clip`, or `None` when empty.
fn clip_box(rect: [i32; 4], clip: [i32; 4]) -> Option<[i32; 4]> {
    let r = [
        rect[0].max(clip[0]),
        rect[1].max(clip[1]),
        rect[2].min(clip[2]),
        rect[3].min(clip[3]),
    ];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

/// Resolves `run` into its drawable pieces, in paint order. A run without
/// [`FieldDecor`] is exactly [`resolve_text`]'s quads (none when it has
/// no inked glyph). A decorated run is scrolled ([`field_scroll`]) and
/// resolves to: the whole line in `run.color`; the selection highlight;
/// the selected glyphs again, clipped to the highlight, in
/// `selected_text`; each preedit underline (one physical-pixel-rounded
/// logical px below the baseline, a `Target` clause twice as thick); and
/// last the caret, [`CARET_WIDTH`] wide (at least one physical px) from
/// the font's ascent to its descent. Every piece is clipped to
/// `run.clip`, and the glyph passes share the frame's one atlas
/// `prepare` (the caller's).
pub fn resolve_run(engine: &mut TextEngine, run: &TextRun, scale_factor: f32) -> Vec<Resolved> {
    let Some(field) = run.field.as_ref() else {
        let quads = resolve_text(engine, run, scale_factor);
        return if quads.is_empty() {
            Vec::new()
        } else {
            vec![Resolved::Glyphs(quads, run.color)]
        };
    };
    let Some(unscrolled) = place(engine, run, scale_factor, 0.0) else {
        return Vec::new();
    };
    let scroll = field_scroll(
        unscrolled.line.width,
        caret_at(&unscrolled.line, field.scroll_anchor),
        run.rect.2,
    );
    let Some(placed) = place(engine, run, scale_factor, scroll) else {
        return Vec::new();
    };
    let clip = clip_phys(run.clip, scale_factor);
    let x_at = |byte: usize| to_phys(placed.origin_x + caret_at(&placed.line, byte), scale_factor);
    let top = to_phys(placed.baseline_logical - placed.line.ascent, scale_factor);
    let bottom = to_phys(placed.baseline_logical + placed.line.descent, scale_factor);
    #[allow(clippy::cast_possible_truncation)]
    let px = |logical: f32| ((logical * scale_factor).round() as i32).max(1);

    let mut out = Vec::new();
    let all = glyph_quads(engine, &placed, clip);
    if !all.is_empty() {
        out.push(Resolved::Glyphs(all, run.color));
    }
    if let Some(selection) = field.selection.as_ref()
        && let Some(highlight) = clip_box(
            [x_at(selection.start), top, x_at(selection.end), bottom],
            clip,
        )
    {
        out.push(Resolved::Rect(highlight, field.selection_fill));
        let inside = glyph_quads(engine, &placed, highlight);
        if !inside.is_empty() {
            out.push(Resolved::Glyphs(inside, field.selected_text));
        }
    }
    for (range, style) in &field.underlines {
        let thickness = match style {
            UnderlineStyle::Plain => px(1.0),
            UnderlineStyle::Target => px(2.0),
        };
        let y0 = placed.baseline.saturating_add(px(1.0));
        let rect = [
            x_at(range.start),
            y0,
            x_at(range.end),
            y0.saturating_add(thickness),
        ];
        if let Some(rect) = clip_box(rect, clip) {
            out.push(Resolved::Rect(rect, field.underline_color));
        }
    }
    if let Some(caret) = field.caret {
        let x0 = x_at(caret);
        let rect = [x0, top, x0.saturating_add(px(CARET_WIDTH)), bottom];
        if let Some(rect) = clip_box(rect, clip) {
            out.push(Resolved::Rect(rect, field.caret_color));
        }
    }
    out
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
        let runs = text_runs(&tree, row, bounds, bounds, None, &theme, &scales);
        let run = runs.first();
        let pad = scales.spacing.sm as f32;
        assert!(
            run.is_some_and(|r| (r.rect.3 - row_height(&scales)).abs() < 1e-6
                && (r.rect.0 - pad).abs() < 1e-6
                && r.color == rgba(theme.text.primary, 1.0))
        );
        ok(set_tree_item_selected(&mut tree, row, true));
        let runs = text_runs(&tree, row, bounds, bounds, None, &theme, &scales);
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
            .flat_map(|row| text_runs(tree, *row, bounds, bounds, None, theme, scales))
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
            text_runs(tree, id, bounds, bounds, None, &theme, &scales)
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
            field: None,
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

/// Text fields, the command palette's query and rows, tooltips and
/// dialog messages (0.133.0).
#[cfg(test)]
#[allow(clippy::float_cmp)]
mod field_tests {
    use std::time::{Duration, Instant};

    use super::{
        CARET_WIDTH, FieldDecor, HAlign, Resolved, TextRun, field_scroll, label_style, resolve_run,
        text_runs, to_phys,
    };
    use crate::paint::{PaintOp, paint_widget_ops_focused, paint_widget_ops_frame};
    use crate::tree::{WidgetId, WidgetTree};
    use crate::widgets::{
        CommandEntry, DialogAction, Tooltip, UnderlineStyle, WidgetKind, command_palette_state,
        insert_button, insert_command_palette, insert_dialog, insert_text_field, new_tree,
        set_command_palette_query, set_text_field_disabled, test_scales, with_text_field_mut,
    };
    use aurora_core::Rect;
    use aurora_text::TextEngine;
    use aurora_theme::{Palette, Scales, Theme, ThemeSet};

    fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn dark_theme() -> Theme {
        let palette = ok(Palette::from_toml_str(include_str!(
            "../../../design/tokens/palette.toml"
        )));
        let mut themes = ThemeSet::new();
        ok(themes.register(include_str!("../../../design/themes/dark.toml")));
        ok(themes.resolve("Dark", &palette))
    }

    fn rgba(color: aurora_theme::Color, a: f32) -> [f32; 4] {
        let [r, g, b] = color.to_srgb_f32();
        [r, g, b, a]
    }

    fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// A text field holding `content`, laid out at (20, 10) 120x24.
    fn field(content: &str) -> (WidgetTree<WidgetKind>, WidgetId, Scales) {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        ok(tree.set_bounds(root, rect(0, 0, 400, 300)));
        let id = ok(insert_text_field(&mut tree, root, &scales, "Name", content));
        ok(tree.set_bounds(id, rect(20, 10, 120, 24)));
        (tree, id, scales)
    }

    fn field_run(
        tree: &WidgetTree<WidgetKind>,
        id: WidgetId,
        focused: Option<WidgetId>,
    ) -> Option<TextRun> {
        let bounds = tree.bounds(id)?;
        text_runs(
            tree,
            id,
            bounds,
            bounds,
            focused,
            &dark_theme(),
            &test_scales(),
        )
        .into_iter()
        .next()
    }

    fn decor(run: &TextRun) -> &FieldDecor {
        match run.field.as_ref() {
            Some(decor) => decor,
            None => unreachable!("a text field's run carries decor"),
        }
    }

    #[test]
    fn a_focused_field_draws_its_caret_at_the_cursor_and_only_then() {
        let (mut tree, id, _) = field("Hello");
        ok(with_text_field_mut(&mut tree, id, |s| s.cursor = 2));
        let Some(run) = field_run(&tree, id, Some(id)) else {
            unreachable!("a field with content draws");
        };
        assert_eq!(run.text, "Hello");
        assert_eq!(decor(&run).caret, Some(2));
        assert_eq!(decor(&run).scroll_anchor, 2);

        // Unfocused (or another widget focused): no caret, but the same
        // scroll anchor, so the field does not jump.
        for focused in [None, Some(tree.root())] {
            let Some(run) = field_run(&tree, id, focused) else {
                unreachable!("content still draws unfocused");
            };
            assert_eq!(decor(&run).caret, None, "{focused:?}");
            assert_eq!(decor(&run).scroll_anchor, 2);
        }

        // Disabled: no caret even when focused.
        ok(set_text_field_disabled(&mut tree, id, true));
        let Some(run) = field_run(&tree, id, Some(id)) else {
            unreachable!("a disabled field still shows its content");
        };
        assert_eq!(decor(&run).caret, None);
    }

    #[test]
    fn field_content_is_text_primary_dimmed_when_disabled_and_clipped_to_the_padding() {
        let (mut tree, id, scales) = field("Hello");
        let theme = dark_theme();
        let Some(run) = field_run(&tree, id, None) else {
            unreachable!()
        };
        assert_eq!(run.color, rgba(theme.text.primary, 1.0));
        let pad = scales.spacing.sm;
        // Inset horizontally by the padding and vertically by the
        // control outline, so decor never covers the border rows.
        assert_eq!(run.clip, rect(20 + i64::from(pad), 11, 120 - 2 * pad, 22));
        assert_eq!(run.rect, (32.0, 10.0, 96.0, 24.0));
        assert_eq!(run.align, HAlign::Start);
        let d = decor(&run);
        assert_eq!(d.caret_color, rgba(theme.text.primary, 1.0));
        assert_eq!(d.selection_fill, rgba(theme.accent.primary, 1.0));
        assert_eq!(d.selected_text, rgba(theme.text.on_accent, 1.0));

        ok(set_text_field_disabled(&mut tree, id, true));
        let Some(run) = field_run(&tree, id, None) else {
            unreachable!()
        };
        assert_eq!(
            run.color,
            rgba(theme.text.primary, theme.state.disabled_opacity)
        );
    }

    #[test]
    fn an_empty_field_draws_only_when_it_has_a_caret() {
        let (tree, id, _) = field("");
        assert!(field_run(&tree, id, None).is_none());
        let Some(run) = field_run(&tree, id, Some(id)) else {
            unreachable!("an empty focused field still has a caret");
        };
        assert_eq!(decor(&run).caret, Some(0));
    }

    #[test]
    fn the_selection_is_the_same_range_from_either_anchor_side() {
        let (mut tree, id, _) = field("Hello world");
        for (cursor, anchor) in [(2, 7), (7, 2)] {
            ok(with_text_field_mut(&mut tree, id, |s| {
                s.cursor = cursor;
                s.selection_anchor = Some(anchor);
            }));
            let Some(run) = field_run(&tree, id, Some(id)) else {
                unreachable!()
            };
            assert_eq!(decor(&run).selection, Some(2..7));
            assert_eq!(decor(&run).caret, Some(cursor));
        }
        // An empty selection (anchor == cursor) draws no highlight.
        ok(with_text_field_mut(&mut tree, id, |s| {
            s.selection_anchor = Some(s.cursor);
        }));
        let Some(run) = field_run(&tree, id, Some(id)) else {
            unreachable!()
        };
        assert_eq!(decor(&run).selection, None);
    }

    #[test]
    fn field_scroll_is_zero_while_the_line_fits_and_pins_the_caret_otherwise() {
        assert_eq!(field_scroll(50.0, 50.0, 100.0), 0.0);
        assert_eq!(field_scroll(99.0, 99.0, 100.0), 0.0);
        // Fits, but a caret after it would not: scroll by the shortfall
        // (RT133-01).
        assert_eq!(field_scroll(100.0, 100.0, 100.0), CARET_WIDTH);
        assert_eq!(field_scroll(99.5, 99.5, 100.0), 0.5);
        // Overflowing, caret at the end: its right edge on the box's.
        let s = field_scroll(300.0, 300.0, 100.0);
        assert_eq!(s, 300.0 + CARET_WIDTH - 100.0);
        assert!(300.0 - s >= 100.0 - CARET_WIDTH && 300.0 - s <= 100.0);
        // Caret near the start: no scroll (clamped at zero).
        assert_eq!(field_scroll(300.0, 10.0, 100.0), 0.0);
        // Middle: the least scroll keeping the caret inside.
        assert_eq!(
            field_scroll(300.0, 150.0, 100.0),
            150.0 + CARET_WIDTH - 100.0
        );
        for bad in [f32::NAN, f32::INFINITY] {
            assert_eq!(field_scroll(bad, 1.0, 100.0), 0.0);
            assert_eq!(field_scroll(300.0, bad, 100.0), 0.0);
            assert_eq!(field_scroll(300.0, 1.0, bad), 0.0);
        }
    }

    fn a_field_run(text: &str, width: f32, decor: FieldDecor) -> TextRun {
        TextRun {
            text: text.to_owned(),
            style: label_style(&test_scales()),
            color: [1.0, 1.0, 1.0, 1.0],
            rect: (10.0, 10.0, width, 24.0),
            align: HAlign::Start,
            clip: Rect {
                x: 10,
                y: 10,
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                width: width as u32,
                height: 24,
            },
            field: Some(decor),
        }
    }

    const PRIMARY: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const CARET: [f32; 4] = [0.9, 0.9, 0.9, 1.0];
    const FILL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    const ON_FILL: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
    const UNDERLINE: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

    fn plain_decor(cursor: usize) -> FieldDecor {
        FieldDecor {
            scroll_anchor: cursor,
            caret: Some(cursor),
            caret_color: CARET,
            selection: None,
            selection_fill: FILL,
            selected_text: ON_FILL,
            underlines: Vec::new(),
            underline_color: UNDERLINE,
        }
    }

    fn rects(pieces: &[Resolved]) -> Vec<([i32; 4], [f32; 4])> {
        pieces
            .iter()
            .filter_map(|p| match p {
                Resolved::Rect(r, c) => Some((*r, *c)),
                Resolved::Glyphs(..) => None,
            })
            .collect()
    }

    #[test]
    fn a_selection_resolves_line_then_fill_then_selected_glyphs_then_caret() {
        let mut engine = ok(TextEngine::new());
        let mut decor = plain_decor(4);
        decor.selection = Some(1..4);
        let pieces = resolve_run(&mut engine, &a_field_run("Hello", 200.0, decor), 1.0);
        let kinds: Vec<_> = pieces
            .iter()
            .map(|p| match p {
                Resolved::Glyphs(_, c) => ("glyphs", *c),
                Resolved::Rect(_, c) => ("rect", *c),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("glyphs", PRIMARY),
                ("rect", FILL),
                ("glyphs", ON_FILL),
                ("rect", CARET),
            ]
        );
        // The selected pass is clipped to the highlight.
        let (Some(Resolved::Rect(fill, _)), Some(Resolved::Glyphs(inside, _))) =
            (pieces.get(1), pieces.get(2))
        else {
            unreachable!("order asserted above");
        };
        for quad in inside {
            let [x0, y0, x1, y1] = quad.dst;
            assert!(x0 >= fill[0] && x1 <= fill[2] && y0 >= fill[1] && y1 <= fill[3]);
        }
    }

    #[test]
    fn the_caret_sits_on_the_snapped_caret_x_and_is_one_logical_px_wide() {
        let mut engine = ok(TextEngine::new());
        for scale in [1.0_f32, 2.0] {
            let run = a_field_run("Hello", 200.0, plain_decor(2));
            let line = engine.shape("Hello", &run.style, scale);
            let Some(x) = line.caret_x(2) else {
                unreachable!("2 is a boundary of Hello");
            };
            let pieces = resolve_run(&mut engine, &run, scale);
            let [(caret, color)] = rects(&pieces)[..] else {
                unreachable!("exactly one rect: the caret");
            };
            assert_eq!(color, CARET);
            assert_eq!(caret[0], to_phys(10.0 + x, scale), "scale {scale}");
            #[allow(clippy::cast_possible_truncation)]
            let width = (CARET_WIDTH * scale).round() as i32;
            assert_eq!(caret[2] - caret[0], width, "scale {scale}");
            assert!(caret[3] > caret[1]);
        }
    }

    #[test]
    fn an_overflowing_field_keeps_its_caret_inside_and_draws_nothing_outside_its_clip() {
        let mut engine = ok(TextEngine::new());
        let text = "The quick brown fox jumps over the lazy dog";
        for scale in [1.0_f32, 2.0] {
            let run = a_field_run(text, 60.0, plain_decor(text.len()));
            let clip = [
                to_phys(10.0, scale),
                to_phys(10.0, scale),
                to_phys(70.0, scale),
                to_phys(34.0, scale),
            ];
            let pieces = resolve_run(&mut engine, &run, scale);
            let Some(&(caret, _)) = rects(&pieces).last() else {
                unreachable!("a caret is drawn");
            };
            assert_eq!(caret[2] - caret[0], to_phys(CARET_WIDTH, scale));
            assert!(caret[2] <= clip[2] && caret[0] >= clip[2] - to_phys(2.0, scale));
            let mut glyphs = 0;
            for piece in &pieces {
                let (r, _) = match piece {
                    Resolved::Rect(r, c) => (vec![*r], c),
                    Resolved::Glyphs(q, c) => (q.iter().map(|q| q.dst).collect(), c),
                };
                for [x0, y0, x1, y1] in r {
                    glyphs += 1;
                    assert!(x0 >= clip[0] && y0 >= clip[1] && x1 <= clip[2] && y1 <= clip[3]);
                }
            }
            assert!(glyphs > 2, "the tail of the line is visible");
            // Scrolled: the first glyph ("T") is not drawn at the start.
            let unscrolled = resolve_run(
                &mut engine,
                &TextRun {
                    field: Some(plain_decor(0)),
                    ..run.clone()
                },
                scale,
            );
            assert_ne!(pieces.first(), unscrolled.first());
        }
    }

    #[test]
    fn a_preedit_is_spliced_in_at_the_cursor_and_underlined_target_thicker() {
        let (mut tree, id, _) = field("ab");
        ok(with_text_field_mut(&mut tree, id, |s| {
            s.cursor = 1;
            s.set_composition("xyz", Some((1, 2)));
        }));
        let Some(run) = field_run(&tree, id, Some(id)) else {
            unreachable!()
        };
        assert_eq!(run.text, "axyzb");
        let d = decor(&run);
        assert_eq!(d.caret, Some(4), "at the preedit's end");
        assert_eq!(d.selection, None);
        assert_eq!(
            d.underlines,
            vec![
                (1..2, UnderlineStyle::Plain),
                (2..3, UnderlineStyle::Target),
                (3..4, UnderlineStyle::Plain),
            ]
        );
        let mut engine = ok(TextEngine::new());
        let pieces = resolve_run(&mut engine, &run, 2.0);
        let rects = rects(&pieces);
        assert_eq!(rects.len(), 4, "three underlines and the caret");
        let height = |i: usize| rects.get(i).map(|(r, _)| r[3] - r[1]);
        assert_eq!(height(0), Some(2), "plain: one logical px at scale 2");
        assert_eq!(height(1), Some(4), "target: twice as thick");
        assert_eq!(height(2), Some(2));
    }

    #[test]
    fn a_control_character_keeps_every_byte_offset() {
        let (mut tree, id, _) = field("a\u{85}b");
        ok(with_text_field_mut(&mut tree, id, |s| s.cursor = 3));
        let Some(run) = field_run(&tree, id, Some(id)) else {
            unreachable!()
        };
        assert_eq!(run.text, "a  b");
        assert_eq!(decor(&run).caret, Some(3));
    }

    #[test]
    fn paint_widget_ops_focused_never_draws_a_caret_frame_does_for_the_focused_field() {
        let (tree, id, scales) = field("Hello");
        let theme = dark_theme();
        let caret = |ops: &[PaintOp]| {
            ops.iter().any(|op| {
                matches!(op, PaintOp::Text(run) if run.field.as_ref().is_some_and(|f| f.caret.is_some()))
            })
        };
        let plain = ok(paint_widget_ops_focused(
            &tree, id, None, &theme, &scales, 1.0,
        ));
        let unfocused = ok(paint_widget_ops_frame(
            &tree, id, None, None, &theme, &scales, 1.0,
        ));
        assert_eq!(plain, unfocused);
        assert!(!caret(&plain));
        let focused = ok(paint_widget_ops_frame(
            &tree,
            id,
            None,
            Some(id),
            &theme,
            &scales,
            1.0,
        ));
        assert!(caret(&focused));
    }

    #[test]
    fn a_tooltip_draws_its_accessibility_label_and_follows_set_text() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        let theme = dark_theme();
        let owner = ok(insert_button(&mut tree, root, &scales, "Owner"));
        let mut tooltip = ok(Tooltip::new(
            &tree,
            owner,
            &scales,
            "Brush size",
            Duration::from_millis(500),
        ));
        let t0 = Instant::now();
        ok(tooltip.set_hover(&mut tree, true, false, t0));
        ok(tooltip.tick(&mut tree, t0 + Duration::from_secs(1)));
        let Some(node) = tooltip.node() else {
            unreachable!("shown after its delay");
        };
        let bounds = rect(0, 30, 120, 20);
        let runs = |tree: &WidgetTree<WidgetKind>| {
            text_runs(tree, node, bounds, bounds, None, &theme, &scales)
        };
        let [run] = &runs(&tree)[..] else {
            unreachable!("one run");
        };
        assert_eq!(run.text, "Brush size");
        assert_eq!(run.color, rgba(theme.text.primary, 1.0));
        assert_eq!(run.style.size_px, 11.0, "type.size.xs");
        ok(tooltip.set_text(&mut tree, "Brush hardness"));
        let texts: Vec<String> = runs(&tree).into_iter().map(|r| r.text).collect();
        assert_eq!(texts, vec!["Brush hardness".to_owned()]);
    }

    fn palette() -> (WidgetTree<WidgetKind>, WidgetId) {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let palette = ok(insert_command_palette(
            &mut tree,
            root,
            vec![
                CommandEntry::new("a", "Undo"),
                CommandEntry::new("b", "Redo"),
                CommandEntry::new("c", "Toggle Layers"),
            ],
        ));
        (tree, palette)
    }

    fn texts_of(
        tree: &WidgetTree<WidgetKind>,
        id: WidgetId,
        focused: Option<WidgetId>,
    ) -> Vec<TextRun> {
        let bounds = rect(0, 0, 300, 24);
        text_runs(
            tree,
            id,
            bounds,
            bounds,
            focused,
            &dark_theme(),
            &test_scales(),
        )
    }

    #[test]
    fn palette_rows_draw_their_titles_in_result_order_selected_on_accent() {
        let (mut tree, palette) = palette();
        let theme = dark_theme();
        let rows = ok(command_palette_state(&tree, palette)).rows().to_vec();
        let titles: Vec<(String, [f32; 4])> = rows
            .iter()
            .flat_map(|row| texts_of(&tree, *row, None))
            .map(|r| (r.text, r.color))
            .collect();
        let on_accent = rgba(theme.text.on_accent, 1.0);
        let primary = rgba(theme.text.primary, 1.0);
        assert_eq!(
            titles,
            vec![
                ("Undo".to_owned(), on_accent),
                ("Redo".to_owned(), primary),
                ("Toggle Layers".to_owned(), primary),
            ]
        );
        ok(set_command_palette_query(&mut tree, palette, "o"));
        ok(set_command_palette_query(&mut tree, palette, "red"));
        let state = ok(command_palette_state(&tree, palette));
        let titles: Vec<String> = state
            .rows()
            .iter()
            .flat_map(|row| texts_of(&tree, *row, None))
            .map(|r| r.text)
            .collect();
        assert_eq!(titles, vec!["Redo".to_owned()]);
    }

    #[test]
    fn the_palette_query_strip_draws_the_query_with_a_caret_while_the_palette_is_focused() {
        let (mut tree, palette) = palette();
        let theme = dark_theme();
        ok(set_command_palette_query(&mut tree, palette, "zzz"));
        let state = ok(command_palette_state(&tree, palette));
        assert!(state.rows().is_empty(), "nothing matches");
        let strip = state.query_strip();
        let [run] = &texts_of(&tree, strip, Some(palette))[..] else {
            unreachable!("the strip draws with zero results");
        };
        assert_eq!(run.text, "zzz");
        assert_eq!(run.color, rgba(theme.text.primary, 1.0));
        assert_eq!(decor(run).caret, Some(3));
        let [run] = &texts_of(&tree, strip, None)[..] else {
            unreachable!("the query still draws unfocused");
        };
        assert_eq!(decor(run).caret, None);
        // An empty query with the palette focused is just a caret.
        ok(set_command_palette_query(&mut tree, palette, ""));
        let [run] = &texts_of(&tree, strip, Some(palette))[..] else {
            unreachable!("an empty focused query still has a caret");
        };
        assert_eq!(decor(run).caret, Some(0));
        // The body and the root themselves draw nothing.
        let body = ok(command_palette_state(&tree, palette)).body();
        assert!(texts_of(&tree, body, Some(palette)).is_empty());
        assert!(texts_of(&tree, palette, Some(palette)).is_empty());
    }

    #[test]
    fn a_dialog_draws_its_message_in_text_primary_and_nothing_on_its_root() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        let theme = dark_theme();
        let handle = ok(insert_dialog(
            &mut tree,
            root,
            &scales,
            "Title",
            "Something happened.",
            vec![DialogAction::new("ok", "OK")],
        ));
        let [run] = &texts_of(&tree, handle.message, None)[..] else {
            unreachable!("the message draws");
        };
        assert_eq!(run.text, "Something happened.");
        assert_eq!(run.color, rgba(theme.text.primary, 1.0));
        assert!(run.field.is_none());
        assert!(texts_of(&tree, handle.root, None).is_empty());
    }

    /// A text field holding `content`, laid out by the real layout
    /// (`insert_text_field`'s own style) in a 200 px column at (0, 0),
    /// focused, with `f` applied to its state; returns its bounds and
    /// resolved pieces at `scale`.
    fn laid_out_field(
        content: &str,
        scale: f32,
        f: impl FnOnce(&mut crate::widgets::TextFieldState),
    ) -> (Rect, Vec<Resolved>) {
        let (mut tree, root) = new_tree(taffy::Style {
            flex_direction: taffy::FlexDirection::Column,
            size: taffy::Size {
                width: taffy::style_helpers::length(200.0_f32),
                height: taffy::style_helpers::auto(),
            },
            ..Default::default()
        });
        let scales = test_scales();
        let id = ok(insert_text_field(&mut tree, root, &scales, "Name", content));
        ok(with_text_field_mut(&mut tree, id, f));
        tree.compute_layout(200.0, 100.0);
        let Some(bounds) = tree.bounds(id) else {
            unreachable!("laid out")
        };
        let Some(run) = field_run(&tree, id, Some(id)) else {
            unreachable!("a field draws a run: {bounds:?}")
        };
        let mut engine = ok(TextEngine::new());
        (bounds, resolve_run(&mut engine, &run, scale))
    }

    #[test]
    fn a_laid_out_field_is_one_row_tall_and_its_decor_stays_inside_the_outline() {
        // C1: the field's real layout height is `row_height`, so the
        // caret, the selection and both preedit underline styles fit
        // strictly inside the 1 px outline's inner rows at every scale.
        let scales = test_scales();
        for scale in [1.0_f32, 1.5, 2.0] {
            // Selection + caret.
            let (bounds, pieces) = laid_out_field("Hello gyp", scale, |s| {
                s.cursor = 9;
                s.selection_anchor = Some(0);
            });
            #[allow(clippy::cast_precision_loss)]
            let (y, h) = (bounds.y as f32, bounds.height as f32);
            assert_eq!(h, crate::widgets::row_height(&scales), "scale {scale}");
            let inner_top = to_phys(y + 1.0, scale);
            let inner_bottom = to_phys(y + h - 1.0, scale);
            let solids = rects(&pieces);
            assert_eq!(solids.len(), 2, "selection and caret, scale {scale}");
            for (r, _) in &solids {
                assert!(
                    r[1] >= inner_top && r[3] <= inner_bottom,
                    "{r:?} outside rows {inner_top}..{inner_bottom} at scale {scale}"
                );
            }
            // The caret spans the font's full content area (not clipped
            // shorter than ascent + descent by a too-short box).
            let mut engine = ok(TextEngine::new());
            let line = engine.shape("Hello gyp", &label_style(&scales), scale);
            let Some(&(caret, _)) = solids.last() else {
                unreachable!()
            };
            let top = to_phys(
                y + h / 2.0 + (line.ascent - line.descent) / 2.0 - line.ascent,
                scale,
            );
            let bottom = to_phys(
                y + h / 2.0 + (line.ascent - line.descent) / 2.0 + line.descent,
                scale,
            );
            assert_eq!((caret[1], caret[3]), (top, bottom), "scale {scale}");

            // Preedit: a Target clause is thicker than a Plain one, and
            // both sit one logical px below the baseline, inside the rows.
            let (_, pieces) = laid_out_field("ab", scale, |s| {
                s.cursor = 2;
                s.set_composition("nihao", Some((2, 5)));
            });
            let underlines: Vec<[i32; 4]> = rects(&pieces)
                .into_iter()
                .filter(|(_, c)| *c == rgba(dark_theme().text.primary, 1.0))
                .map(|(r, _)| r)
                .collect();
            // Plain "ni", Target "hao", then the caret (same colour).
            let [plain, target, _caret] = underlines[..] else {
                unreachable!("two underlines and a caret: {underlines:?}")
            };
            assert!(
                target[3] - target[1] > plain[3] - plain[1],
                "Target {target:?} not thicker than Plain {plain:?} at scale {scale}"
            );
            let baseline = to_phys(y + h / 2.0 + (line.ascent - line.descent) / 2.0, scale);
            #[allow(clippy::cast_possible_truncation)]
            let one = (scale.round() as i32).max(1);
            assert_eq!(plain[1], baseline + one, "C2: y offset, scale {scale}");
            assert_eq!(target[1], baseline + one, "C2: y offset, scale {scale}");
            for r in [plain, target] {
                assert!(
                    r[1] >= inner_top && r[3] <= inner_bottom,
                    "{r:?} at {scale}"
                );
            }
        }
    }

    #[test]
    fn a_caret_after_a_line_that_nearly_fills_the_field_stays_visible() {
        // RT133-01: sweep the inner width around the line's own width at
        // several scales; an end-of-line caret must always be drawn.
        let scales = test_scales();
        let pad = scales.spacing.sm;
        let mut engine = ok(TextEngine::new());
        let mut checked = 0;
        for scale in [1.0_f32, 1.5, 2.0] {
            for content in ["Hello", "Layer 12", "gauntlet", "Wwww", "Aurora image", "x"] {
                let line = engine.shape(content, &label_style(&scales), scale);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let ceil = line.width.ceil() as u32;
                for inner in ceil.saturating_sub(1)..=ceil + 2 {
                    let (mut tree, root) = new_tree(taffy::Style::default());
                    ok(tree.set_bounds(root, rect(0, 0, 400, 300)));
                    let id = ok(insert_text_field(&mut tree, root, &scales, "N", content));
                    ok(tree.set_bounds(id, rect(20, 10, inner + 2 * pad, 24)));
                    let Some(run) = field_run(&tree, id, Some(id)) else {
                        unreachable!()
                    };
                    let clip = super::clip_phys(run.clip, scale);
                    let pieces = resolve_run(&mut engine, &run, scale);
                    let Some(&(caret, _)) = rects(&pieces).last() else {
                        unreachable!("no caret: {content:?} inner {inner} scale {scale}");
                    };
                    assert!(
                        caret[0] >= clip[0] && caret[2] <= clip[2] && caret[2] > caret[0],
                        "{content:?} inner {inner} scale {scale}: {caret:?} vs {clip:?}"
                    );
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 3 * 6 * 4);
    }

    #[test]
    fn a_caret_outside_its_clip_is_dropped_and_one_straddling_it_is_cut() {
        // M10: a focused field partially clipped by an ancestor.
        let (tree, id, _) = field("Hi");
        let Some(bounds) = tree.bounds(id) else {
            unreachable!()
        };
        let theme = dark_theme();
        let scales = test_scales();
        let caret_color = rgba(theme.text.primary, 1.0);
        let runs_in = |clip: Rect| {
            let run = text_runs(&tree, id, bounds, clip, Some(id), &theme, &scales);
            let mut engine = ok(TextEngine::new());
            run.iter()
                .flat_map(|r| resolve_run(&mut engine, r, 1.0))
                .collect::<Vec<_>>()
        };
        // Clip ends just right of the padding: "Hi"'s end caret is past it.
        let left = runs_in(rect(20, 10, 14, 24));
        assert!(
            rects(&left).iter().all(|(_, c)| *c != caret_color),
            "a caret outside the clip is not drawn"
        );
        // Clip is the field's lower half: the caret is cut, not dropped.
        let lower = runs_in(rect(20, 22, 120, 12));
        let Some(&(caret, _)) = rects(&lower).last() else {
            unreachable!("the caret straddles the clip")
        };
        assert_eq!(caret[1], 22);
        assert!(caret[3] <= 33 && caret[3] > 22);
    }

    #[test]
    fn a_non_char_boundary_cursor_or_selection_snaps_to_the_previous_boundary() {
        // C6/C8: "é" is two bytes; byte 1 is inside it.
        let (mut tree, id, _) = field("aé");
        ok(with_text_field_mut(&mut tree, id, |s| {
            s.cursor = 2;
            s.selection_anchor = None;
        }));
        let Some(run) = field_run(&tree, id, Some(id)) else {
            unreachable!()
        };
        assert_eq!(decor(&run).caret, Some(1), "previous boundary, not len");
        ok(with_text_field_mut(&mut tree, id, |s| {
            s.cursor = 99;
            s.selection_anchor = Some(2);
        }));
        let Some(run) = field_run(&tree, id, Some(id)) else {
            unreachable!()
        };
        assert_eq!(decor(&run).caret, Some(3), "past the end is the end");
        assert_eq!(decor(&run).selection, Some(1..3));
    }

    #[test]
    fn the_query_caret_follows_focus_anywhere_inside_the_palette_only() {
        // M15a/M15b/C4.
        let (mut tree, palette) = palette();
        ok(set_command_palette_query(&mut tree, palette, "o"));
        let state = ok(command_palette_state(&tree, palette));
        let strip = state.query_strip();
        let Some(&row) = state.rows().first() else {
            unreachable!("\"o\" matches Undo")
        };
        let other = ok(insert_button(&mut tree, palette, &test_scales(), "x"));
        let Some(unrelated) = tree.parent(palette) else {
            unreachable!()
        };
        let other_root = ok(insert_button(&mut tree, unrelated, &test_scales(), "y"));
        let caret = |focused| {
            texts_of(&tree, strip, focused)
                .first()
                .and_then(|r| r.field.as_ref())
                .and_then(|d| d.caret)
        };
        assert_eq!(caret(Some(palette)), Some(1));
        assert_eq!(caret(Some(row)), Some(1), "focus on a palette row");
        assert_eq!(caret(Some(other)), Some(1), "any descendant counts");
        assert_eq!(caret(Some(other_root)), None, "an unrelated widget");
        assert_eq!(caret(None), None);
    }
}
