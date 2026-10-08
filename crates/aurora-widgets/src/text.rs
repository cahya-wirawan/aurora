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
//! caret while focused ([`CARET_WIDTH`]; blinking since 0.139.0,
//! [`crate::CaretBlink`]), its selection (`accent.primary` highlight,
//! selected glyphs redrawn in `text.on_accent`) and its IME preedit
//! spliced in at the cursor and underlined ([`FieldDecor`],
//! [`resolve_run`]), horizontally scrolled
//! to keep the caret visible (since 0.138.0's review a text field's
//! scroll is sticky and stored — [`sticky_scroll`],
//! [`update_field_scrolls`]; the palette query stays caret-pinned,
//! [`field_scroll`]) — the command palette's query strip (query plus caret) and
//! result rows, a tooltip's text (its accessibility label, the one copy
//! `Tooltip::set_text` keeps current), and a dialog's message (one
//! line). Since 0.140.0 a measured `Checkbox`'s label, since 0.141.0 a
//! dialog's title, and since 0.142.0 a docked panel's title (its root's
//! label, drawn in the root's first child — [`panel_title`]). **Not
//! yet**: a text field's placeholder (`TextFieldState` has none).
//!
//! **Overflow (0.142.0).** There is no wrapping. A run is clipped to its
//! box by default ([`TextOverflow::Clip`]); a static `Label`, a
//! `TreeItem`'s label and a panel title instead end in "…" when they do
//! not fit ([`TextOverflow::Ellipsis`]), cut on a grapheme boundary —
//! a label cut mid-letter ("No radius for Marque") reads as a typo, not
//! as truncation. Only what is *drawn* is shortened: every accessibility
//! label and stored string stays complete. Editable lines (text fields,
//! the palette query) always clip, so their caret, selection and scroll
//! maths index the real text.

use std::ops::Range;

use aurora_core::Rect;
use aurora_text::{GlyphKey, ShapedLine, TextEngine, TextStyle, snap_glyph_origin};
use aurora_theme::{Color, Scales, Theme};

use crate::tree::{WidgetId, WidgetTree};
use crate::widgets::{
    TextFieldState, UnderlineStyle, WidgetKind, checkbox_metrics, composition_segments,
    floor_char_boundary, row_height,
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
    /// `rect` ([`FieldDecor::scroll`]).
    pub field: Option<FieldDecor>,
    /// What a line wider than `rect` does (0.142.0): clip (the default,
    /// and always for a run with [`Self::field`]) or end in "…".
    pub overflow: TextOverflow,
}

/// What a [`TextRun`] wider than its box does (0.142.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextOverflow {
    /// Drawn whole and clipped at [`TextRun::clip`].
    #[default]
    Clip,
    /// The longest grapheme-boundary prefix that fits with a trailing
    /// "…" is drawn instead (just "…", clipped, if even that does not
    /// fit). Ignored for a run with [`TextRun::field`]: an editable
    /// line's offsets index its real text.
    Ellipsis,
}

/// The mark an ellipsized run ends in (U+2026, in the bundled Inter).
const ELLIPSIS: &str = "\u{2026}";

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
/// records. Its width and colour (`text.primary`, provisional) are
/// flagged to the design owner (Cahya, PRD FR-027 *Ownership*) rather
/// than invented here, as is its blink cadence
/// ([`crate::CARET_BLINK_INTERVAL`], 0.139.0).
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
    /// How the line scrolls (0.138.0 review). `Some(previous)` — a text
    /// field's stored [`TextFieldState::scroll`] — is **sticky**
    /// ([`sticky_scroll`]): kept until the caret would leave the box.
    /// `None` — the command palette's query, whose caret is always at the
    /// end of its line — is **caret-pinned** ([`field_scroll`]).
    pub scroll: Option<f32>,
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
            if let Some(WidgetKind::CommandPalette(state)) =
                tree.parent(parent).and_then(|root| tree.payload(root))
            {
                return state.row_title(row).map(|title| (title.to_owned(), true));
            }
            // A plain list row that opted in (0.147.1,
            // `ListRowState::draws_label`; `aurora-ui`'s History panel):
            // its own accessible name is its text. The read-only
            // Properties rows do not opt in -- the tool-controls readout
            // beside them already shows their text -- and stay undrawn.
            // Enabled-ness is the payload's own `disabled`, which the
            // caller folds in.
            let Some(WidgetKind::ListRow(crate::widgets::ListRowState {
                draws_label: true, ..
            })) = tree.payload(row)
            else {
                return None;
            };
            let label = tree.accessibility(row)?.label()?;
            Some((label.to_owned(), true))
        }
        _ => None,
    }
}

/// The intersection of two rects, or `None` when they do not overlap.
pub(crate) fn intersect(a: Rect, b: Rect) -> Option<Rect> {
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
        scroll: None,
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
        decor.scroll = Some(state.scroll());
        decor.caret = show_caret.then_some(end);
        decor.underlines = composition_segments(composition)
            .into_iter()
            .map(|(range, style)| (range.start + cursor..range.end + cursor, style))
            .collect();
        return (sanitize_field(&text), decor);
    }
    let mut decor = decor(theme, cursor);
    decor.scroll = Some(state.scroll());
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
        overflow: TextOverflow::Clip,
    };
    // A label that names something (a layer, a panel, a readout) ends in
    // "…" rather than being cut mid-letter (0.142.0).
    let ellipsized = |run: TextRun| TextRun {
        overflow: TextOverflow::Ellipsis,
        ..run
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
            overflow: TextOverflow::Clip,
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
            vec![ellipsized(run(
                &state.label,
                rgba(color, opacity(state.disabled, theme)),
                inset_x((full.0, full.1, full.2, height), pad),
                HAlign::Start,
            ))]
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
        // A checkbox's label (0.140.0): one line, flush left, just past
        // the box and a `spacing.sm` gap (`checkbox_metrics`), in
        // `text.primary` — it names a control the user acts on, not
        // supporting text — faded by the same disabled opacity the box
        // itself carries. Only a *measured* checkbox (laid out through
        // `crate::compute_text_layout`) draws one: an unmeasured one's
        // bounds are all box (`checkbox_box_rect`), so no run is produced
        // — every golden (laid out with no text engine) stays text-free
        // exactly as before. A measured checkbox squeezed to no room past
        // its box draws none either.
        WidgetKind::Checkbox(state) if tree.is_measured(id) == Some(true) => {
            let (side, gap) = checkbox_metrics(scales);
            let (x, y, w, h) = full;
            let width = w - side - gap;
            if width > 0.0 {
                vec![run(
                    &state.label,
                    rgba(theme.text.primary, opacity(state.disabled, theme)),
                    (x + side + gap, y, width, h),
                    HAlign::Start,
                )]
            } else {
                Vec::new()
            }
        }
        // A static label: one line, flush left, supporting text.
        WidgetKind::Label(state) => {
            let color = if state.disabled {
                theme.text.disabled
            } else {
                theme.text.secondary
            };
            vec![ellipsized(run(
                &state.text,
                rgba(color, 1.0),
                full,
                HAlign::Start,
            ))]
        }
        // A docked panel's title (0.142.0): the panel's first child (its
        // unlabelled title slot) draws the panel `Region`'s own label --
        // the one copy of the title -- one line, flush left, inset by
        // `spacing.sm` like a tree row's label, ellipsized. `text.secondary`
        // (gated at 4.5:1 on `surface.panel`), Regular, body size: the
        // title is a heading over the rows, not one of them, but no
        // heading weight or size exists in the type tokens -- the title's
        // typography and colour are the design owner's call, flagged
        // rather than invented.
        WidgetKind::Container if let Some(title) = panel_title(tree, id) => {
            vec![ellipsized(run(
                title,
                rgba(theme.text.secondary, 1.0),
                inset_x(full, pad),
                HAlign::Start,
            ))]
        }
        // A dialog's title (0.141.0): its presentational first child
        // draws the dialog's own label -- the one copy of the title --
        // on one line, flush left, clipped, in `text.primary`, exactly
        // like the message below it. Regular weight: no heavier title
        // weight exists in the type tokens, and inventing one is the
        // design owner's call.
        WidgetKind::Container if let Some(title) = dialog_title(tree, id) => vec![run(
            title,
            rgba(theme.text.primary, 1.0),
            full,
            HAlign::Start,
        )],
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

/// The title a dialog's title slot draws: its parent `Dialog`'s own
/// accessibility label, when `id` is that dialog's unlabelled
/// `Role::GenericContainer` child (`widgets::dialog::insert_dialog`
/// builds exactly one, first). `None` for any other widget.
fn dialog_title(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Option<&str> {
    let parent = tree.parent(id)?;
    let is_slot = matches!(tree.payload(parent)?, WidgetKind::Dialog)
        && tree.accessibility(id)?.role() == accesskit::Role::GenericContainer;
    is_slot
        .then(|| tree.accessibility(parent)?.label())
        .flatten()
}

/// The title a docked panel's title slot draws: its parent `Panel`'s own
/// accessibility label, when `id` is that panel's unlabelled
/// `Role::GenericContainer` **first** child (`aurora_ui`'s `insert_panel`
/// builds exactly that). The first-child test is load-bearing, unlike the
/// dialog's: a panel's body and its controls strips are unlabelled
/// generic containers too. `None` for any other widget.
fn panel_title(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Option<&str> {
    let parent = tree.parent(id)?;
    let node = tree.accessibility(id)?;
    let is_slot = matches!(tree.payload(parent)?, WidgetKind::Panel)
        && node.role() == accesskit::Role::GenericContainer
        && node.label().is_none()
        && tree.children(parent)?.first() == Some(&id);
    is_slot
        .then(|| tree.accessibility(parent)?.label())
        .flatten()
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
pub(crate) fn sanitize_label(text: &str) -> String {
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
    let line = fitted_line(engine, run, scale_factor).1;
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

/// The text `run` actually draws and its shaped line: `run.text` itself,
/// unless the run is [`TextOverflow::Ellipsis`] (and not an editable
/// line) and strictly wider than its box — then the longest prefix, cut
/// on a grapheme boundary and with trailing whitespace trimmed, that fits
/// with a trailing [`ELLIPSIS`], or the ellipsis alone if no prefix does
/// (drawn clipped when even it is too wide). The prefix is found from the
/// full line's own grapheme carets — the cut whose caret plus the
/// ellipsis's width fits, longest first — and each candidate is reshaped
/// to confirm it, so kerning across the cut can only move it one
/// boundary shorter, never past the box. Every string here is stable
/// from frame to frame, so the engine's shape cache absorbs the reshape.
fn fitted_line<'a>(
    engine: &mut TextEngine,
    run: &'a TextRun,
    scale_factor: f32,
) -> (std::borrow::Cow<'a, str>, std::sync::Arc<ShapedLine>) {
    let line = engine.shape(&run.text, &run.style, scale_factor);
    let width = run.rect.2;
    if run.overflow != TextOverflow::Ellipsis || run.field.is_some() || line.width <= width {
        return (std::borrow::Cow::Borrowed(run.text.as_str()), line);
    }
    let mark = engine.shape(ELLIPSIS, &run.style, scale_factor);
    for &(byte, x) in line.carets.iter().rev() {
        if byte >= run.text.len() || x + mark.width > width {
            continue;
        }
        let Some(prefix) = run.text.get(..byte).map(str::trim_end) else {
            continue;
        };
        if prefix.is_empty() {
            break;
        }
        let candidate = format!("{prefix}{ELLIPSIS}");
        let shaped = engine.shape(&candidate, &run.style, scale_factor);
        if shaped.width <= width {
            return (std::borrow::Cow::Owned(candidate), shaped);
        }
    }
    (std::borrow::Cow::Borrowed(ELLIPSIS), mark)
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
/// to `[0, line_width + CARET_WIDTH - inner_width]`. **Caret-pinned**:
/// recomputed from the caret alone, with no memory of the previous
/// scroll. Since 0.138.0's review only the command palette's query uses
/// it (its caret is always at the end of its line, where pinned and
/// sticky agree); a text field scrolls by [`sticky_scroll`], because a
/// pinned scroll moves the text under a click and runs a drag away.
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

/// A text field's sticky horizontal scroll (logical px, `>= 0`; 0.138.0
/// review): `0` while the line and a caret after it fit (as
/// [`field_scroll`]); otherwise `prev` itself while the caret at
/// `caret_x` lies in the visible window `[prev, prev + inner_width -
/// CARET_WIDTH]`, else the least change that brings it just inside
/// (caret at the window's left edge, or its right edge flush with the
/// box's). Always clamped to `[0, line_width + CARET_WIDTH -
/// inner_width]`, so a line that shrank never leaves blank space at the
/// right. Any non-finite input gives `0`.
///
/// Sticky is what makes pointer placement stable: a click or a drag
/// puts the caret on a byte inside the window, which then does not move
/// the window — the text stays under the pointer.
#[must_use]
pub fn sticky_scroll(prev: f32, line_width: f32, caret_x: f32, inner_width: f32) -> f32 {
    if ![prev, line_width, caret_x, inner_width]
        .iter()
        .all(|v| v.is_finite())
        || line_width + CARET_WIDTH <= inner_width
    {
        return 0.0;
    }
    let upper = (line_width + CARET_WIDTH - inner_width).max(0.0);
    let mut scroll = prev.clamp(0.0, upper);
    if caret_x < scroll {
        scroll = caret_x;
    } else if caret_x + CARET_WIDTH > scroll + inner_width {
        scroll = caret_x + CARET_WIDTH - inner_width;
    }
    scroll.clamp(0.0, upper)
}

/// The scroll `field`'s line takes in a `inner_width`-wide box, given
/// the line placed unscrolled: sticky from a text field's stored scroll,
/// caret-pinned for the palette query ([`FieldDecor::scroll`]).
fn decor_scroll(unscrolled: &Placed, field: &FieldDecor, inner_width: f32) -> f32 {
    let caret_x = caret_at(&unscrolled.line, field.scroll_anchor);
    match field.scroll {
        Some(prev) => sticky_scroll(prev, unscrolled.line.width, caret_x, inner_width),
        None => field_scroll(unscrolled.line.width, caret_x, inner_width),
    }
}

/// `run` shaped and placed at its [`decor_scroll`] — the one placement
/// [`resolve_run`], [`field_offset_at`] and [`update_field_scrolls`]
/// share. `None` for a run without decor or one [`place`] refuses.
fn place_field(engine: &mut TextEngine, run: &TextRun, scale_factor: f32) -> Option<(Placed, f32)> {
    let field = run.field.as_ref()?;
    let unscrolled = place(engine, run, scale_factor, 0.0)?;
    let scroll = decor_scroll(&unscrolled, field, run.rect.2);
    Some((place(engine, run, scale_factor, scroll)?, scroll))
}

/// Updates every visible text field's stored horizontal scroll
/// ([`TextFieldState::scroll`], via [`crate::widgets::set_text_field_scroll`])
/// to its [`sticky_scroll`] for the current content and cursor, shaped
/// at `scale_factor` from exactly the run [`text_runs`] builds (0.138.0
/// review). The app calls it once per frame before collecting widget
/// paints, so what is drawn — and what the next click is mapped against
/// ([`field_offset_at`]) — is the stored value. A field that is not laid
/// out or not visible keeps what it had. Returns how many fields
/// changed (each marked dirty).
pub fn update_field_scrolls(
    engine: &mut TextEngine,
    tree: &mut WidgetTree<WidgetKind>,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> usize {
    let fields: Vec<WidgetId> = tree
        .paint_order()
        .into_iter()
        .filter(|&id| matches!(tree.payload(id), Some(WidgetKind::TextField(_))))
        .collect();
    let mut changed = 0;
    for id in fields {
        let Some(bounds) = tree.bounds(id) else {
            continue;
        };
        let Some(clip) = tree.visible_rect(id, bounds) else {
            continue;
        };
        // Focus draws a caret but never moves the scroll anchor.
        let Some(run) = text_runs(tree, id, bounds, clip, None, theme, scales)
            .into_iter()
            .next()
        else {
            continue;
        };
        let Some((_, scroll)) = place_field(engine, &run, scale_factor) else {
            continue;
        };
        if crate::widgets::set_text_field_scroll(tree, id, scroll).unwrap_or(false) {
            changed += 1;
        }
    }
    changed
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
/// no inked glyph). A decorated run is scrolled ([`FieldDecor::scroll`]) and
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
    let Some((placed, _)) = place_field(engine, run, scale_factor) else {
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

/// The byte offset of text field `id`'s content whose caret lies nearest
/// logical window x `x` (0.138.0) — where a click at `x` puts the caret.
/// Built from exactly the geometry [`resolve_run`] draws: the field's own
/// [`text_runs`] run (`bounds`, clipped to its visible rect), shaped at
/// `scale_factor`, scrolled by the field's stored sticky scroll
/// ([`FieldDecor::scroll`], kept by [`update_field_scrolls`]), so the
/// answer is the caret the user sees under the pointer. `x` is first
/// clamped to the field's padded text box (0.138.0 review, critic C2) —
/// its right edge less [`CARET_WIDTH`], where the rightmost visible
/// caret is drawn — and only carets wholly inside the visible window are
/// candidates (the window [`sticky_scroll`] keeps still), so a press in
/// the padding or at the edge of a scrolled field picks the nearest
/// *visible* caret, never one scrolled or clipped out of view, and
/// placing it never scrolls the text (a drag past the edge therefore
/// stops at the last visible caret: no drag auto-scroll). Nearest by
/// distance to each grapheme boundary's caret; an exact tie goes to the
/// lower byte; a point past either end of a line that fits (or is
/// scrolled to that end) gives `0` or the content's length. `theme` is
/// only the run's colours, which cannot move anything.
///
/// `None` for a widget that is not a text field, is not laid out or
/// visible, or is mid-composition — the drawn text is then the content
/// with the preedit spliced in, so none of its offsets is one into the
/// content. A returned offset is a grapheme boundary of the *drawn*
/// (control-character-sanitized, byte-length-preserving) text, which may
/// split a content cluster such as `"\r\n"`;
/// [`crate::widgets::set_text_field_caret`] floors it.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn field_offset_at(
    engine: &mut TextEngine,
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
    x: f32,
) -> Option<usize> {
    let Some(WidgetKind::TextField(state)) = tree.payload(id) else {
        return None;
    };
    if state.is_composing() || !x.is_finite() {
        return None;
    }
    let bounds = tree.bounds(id)?;
    let clip = tree.visible_rect(id, bounds)?;
    let run = text_runs(tree, id, bounds, clip, Some(id), theme, scales)
        .into_iter()
        .next()?;
    let (placed, scroll) = place_field(engine, &run, scale_factor)?;
    let inner = run.rect.2;
    let left = run.rect.0;
    let x = x.clamp(left, (left + inner - CARET_WIDTH).max(left));
    // Only a caret inside the window `sticky_scroll` keeps still — the
    // same comparison — so placing the pick never moves the text; every
    // caret when none is (a box narrower than one glyph).
    let visible: Vec<(usize, f32)> = placed
        .line
        .carets
        .iter()
        .copied()
        .filter(|&(_, cx)| cx >= scroll && cx + CARET_WIDTH <= scroll + inner)
        .collect();
    let carets = if visible.is_empty() {
        placed.line.carets.as_slice()
    } else {
        visible.as_slice()
    };
    nearest_caret(carets, x - placed.origin_x)
}

/// The byte of the caret in `carets` (ascending `(byte, x)`) nearest `x`,
/// both relative to the line's pen origin; an exact tie goes to the
/// lower byte. `None` for no carets or a non-finite `x`.
fn nearest_caret(carets: &[(usize, f32)], x: f32) -> Option<usize> {
    if !x.is_finite() {
        return None;
    }
    let mut best: Option<(usize, f32)> = None;
    for &(byte, caret_x) in carets {
        let distance = (caret_x - x).abs();
        if best.is_none_or(|(_, nearest)| distance < nearest) {
            best = Some((byte, distance));
        }
    }
    best.map(|(byte, _)| byte)
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
    fn a_label_draws_its_text_flush_left_in_secondary_and_disabled_in_text_disabled() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        let label = ok(crate::widgets::insert_label(
            &mut tree,
            root,
            &scales,
            "Size 24 px",
        ));
        let theme = dark_theme();
        let bounds = Rect {
            x: 10,
            y: 20,
            width: 200,
            height: 21,
        };
        let runs = text_runs(&tree, label, bounds, bounds, None, &theme, &scales);
        assert_eq!(runs.len(), 1);
        assert!(runs.first().is_some_and(|r| r.text == "Size 24 px"
            && r.align == HAlign::Start
            && r.color == rgba(theme.text.secondary, 1.0)
            && r.field.is_none()));
        ok(crate::widgets::set_label_disabled(&mut tree, label, true));
        let runs = text_runs(&tree, label, bounds, bounds, None, &theme, &scales);
        assert_eq!(
            runs.first().map(|r| r.color),
            Some(rgba(theme.text.disabled, 1.0))
        );
        // It paints no solids of its own: the text is everything.
        place(&mut tree, label, 10, 20, 200, 21);
        let paints = ok(crate::paint_widget(&tree, label, &theme, &scales, 1.0));
        assert!(paints.is_empty(), "{paints:?}");
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
            overflow: super::TextOverflow::Clip,
        }
    }

    /// `text` as an ellipsizing run `width` logical px wide at (10, 10).
    fn ellipsis_run(text: &str, width: f32, align: HAlign) -> TextRun {
        TextRun {
            text: text.to_owned(),
            rect: (10.0, 10.0, width, 20.0),
            align,
            overflow: super::TextOverflow::Ellipsis,
            ..a_run(Rect {
                x: -10_000,
                y: -10_000,
                width: 20_000,
                height: 20_000,
            })
        }
    }

    fn fitted(engine: &mut TextEngine, run: &TextRun) -> (String, f32) {
        let (text, line) = super::fitted_line(engine, run, 1.0);
        (text.into_owned(), line.width)
    }

    /// 0.142.0: a label too wide for its box ends in "…", fits the box,
    /// and keeps the *longest* prefix that does — one grapheme more would
    /// not fit. The Properties readout that read "No radius for Marque".
    #[test]
    fn an_ellipsized_run_too_wide_for_its_box_keeps_the_longest_fitting_prefix() {
        let mut engine = engine();
        let full = "No radius for Marquee Select";
        let style = super::label_style(&test_scales());
        let full_width = engine.shape(full, &style, 1.0).width;
        let width = (full_width * 0.6).floor();
        let (text, drawn) = fitted(&mut engine, &ellipsis_run(full, width, HAlign::Start));
        let Some(prefix) = text.strip_suffix('\u{2026}') else {
            unreachable!("ends in an ellipsis: {text:?}");
        };
        assert!(!prefix.is_empty() && full.starts_with(prefix), "{text:?}");
        assert!(drawn <= width, "{drawn} <= {width}");
        let rest = full.get(prefix.len()..).unwrap_or("");
        let skipped = rest.len() - rest.trim_start().len();
        let next = rest.trim_start().chars().next().map_or(0, char::len_utf8);
        let longer_prefix = full.get(..prefix.len() + skipped + next).unwrap_or(full);
        let longer = format!("{longer_prefix}\u{2026}");
        assert!(
            engine.shape(&longer, &style, 1.0).width > width,
            "one more grapheme ({longer:?}) would have fitted, so {text:?} is not the longest"
        );
        // The same run clipped draws the whole text: only Ellipsis cuts.
        let clipped = TextRun {
            overflow: super::TextOverflow::Clip,
            ..ellipsis_run(full, width, HAlign::Start)
        };
        assert_eq!(fitted(&mut engine, &clipped).0, full);
        // And what is drawn is that shorter line, not the full one.
        let ellipsized = resolve_text(&mut engine, &ellipsis_run(full, width, HAlign::Start), 1.0);
        let whole = resolve_text(&mut engine, &clipped, 1.0);
        assert!(ellipsized.len() < whole.len());
    }

    /// A run exactly as wide as its box fits: it is drawn unchanged,
    /// glyph for glyph, as is any run with room to spare.
    #[test]
    fn a_fitting_ellipsized_run_is_drawn_unchanged() {
        let mut engine = engine();
        let text = "Background";
        let width = engine
            .shape(text, &super::label_style(&test_scales()), 1.0)
            .width;
        for box_width in [width, width + 40.0] {
            let run = ellipsis_run(text, box_width, HAlign::Start);
            assert_eq!(fitted(&mut engine, &run).0, text, "box {box_width}");
            let clipped = TextRun {
                overflow: super::TextOverflow::Clip,
                ..run.clone()
            };
            assert_eq!(
                resolve_text(&mut engine, &run, 1.0),
                resolve_text(&mut engine, &clipped, 1.0)
            );
        }
    }

    /// A box narrower than the ellipsis itself draws just the ellipsis
    /// (clipped by the run's clip as usual) — no panic, no empty prefix.
    #[test]
    fn a_box_narrower_than_the_ellipsis_draws_only_the_ellipsis() {
        let mut engine = engine();
        // The bundled font really has the mark: one glyph, not `.notdef`.
        let mark = engine.shape("\u{2026}", &super::label_style(&test_scales()), 1.0);
        assert!(mark.width > 0.0);
        assert!(!mark.glyphs.is_empty() && mark.glyphs.iter().all(|g| g.glyph_id != 0));
        for width in [3.0, 0.5, 0.0, -5.0] {
            let run = ellipsis_run("Layer 1", width, HAlign::Start);
            assert_eq!(fitted(&mut engine, &run).0, "\u{2026}", "box {width}");
            let _ = resolve_text(&mut engine, &run, 1.0);
        }
    }

    /// The cut lands on a grapheme boundary of multibyte text: never inside
    /// a UTF-8 sequence, an accent cluster or an emoji ZWJ sequence.
    #[test]
    fn an_ellipsized_multibyte_run_is_cut_on_a_grapheme_boundary() {
        use unicode_segmentation::UnicodeSegmentation;
        let mut engine = engine();
        let full = "Cafe\u{301} \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467} \u{65E5}\u{672C}\u{8A9E} e\u{301}e\u{301}e\u{301}";
        let style = super::label_style(&test_scales());
        let full_width = engine.shape(full, &style, 1.0).width;
        let boundaries: Vec<usize> = full
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain([full.len()])
            .collect();
        let mut step = 1.0;
        while step < full_width {
            let (text, drawn) = fitted(&mut engine, &ellipsis_run(full, step, HAlign::Start));
            if let Some(prefix) = text.strip_suffix('\u{2026}')
                && !prefix.is_empty()
            {
                assert!(full.starts_with(prefix), "{text:?}");
                assert!(
                    boundaries.contains(&prefix.len()),
                    "cut at byte {} is not a grapheme boundary ({text:?})",
                    prefix.len()
                );
                assert!(drawn <= step, "{drawn} <= {step}");
            }
            step += 3.0;
        }
    }

    /// A centred ellipsized run centres the *shortened* line in its box.
    #[test]
    fn a_centred_ellipsized_run_centres_the_shortened_line() {
        let mut engine = engine();
        let run = ellipsis_run("A rather long centred caption", 60.0, HAlign::Center);
        let (_, drawn) = fitted(&mut engine, &run);
        let Some(placed) = super::place(&mut engine, &run, 1.0, 0.0) else {
            unreachable!("finite run");
        };
        assert!((placed.origin_x - (10.0 + (60.0 - drawn) / 2.0)).abs() < 1e-4);
        assert!(
            placed.origin_x >= 10.0,
            "inside the box, not overflowing left"
        );
    }

    /// An editable line never ellipsizes, even if a run claimed it should:
    /// its caret, selection and scroll offsets index the real text.
    #[test]
    fn an_editable_line_is_never_ellipsized() {
        let mut engine = engine();
        let text = "a long field value that overflows its box";
        let run = TextRun {
            field: Some(super::FieldDecor {
                scroll_anchor: 0,
                scroll: None,
                caret: None,
                caret_color: [1.0; 4],
                selection: None,
                selection_fill: [1.0; 4],
                selected_text: [1.0; 4],
                underlines: Vec::new(),
                underline_color: [1.0; 4],
            }),
            ..ellipsis_run(text, 40.0, HAlign::Start)
        };
        assert_eq!(fitted(&mut engine, &run).0, text);
    }

    /// Which widgets ellipsize: a static label and a tree row's label do;
    /// a text field (and every other run) clips.
    #[test]
    fn labels_and_tree_rows_ellipsize_and_text_fields_clip() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        let theme = dark_theme();
        let label = ok(crate::widgets::insert_label(&mut tree, root, &scales, "L"));
        let view = ok(insert_tree_view(&mut tree, root, Some("Layers")));
        let row = ok(insert_tree_item(&mut tree, view, &scales, "Row", false));
        let field = ok(crate::widgets::insert_text_field(
            &mut tree, root, &scales, "Name", "value",
        ));
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 21,
        };
        let overflow = |id| {
            text_runs(&tree, id, bounds, bounds, None, &theme, &scales)
                .first()
                .map(|r| r.overflow)
        };
        assert_eq!(overflow(label), Some(super::TextOverflow::Ellipsis));
        assert_eq!(overflow(row), Some(super::TextOverflow::Ellipsis));
        assert_eq!(overflow(field), Some(super::TextOverflow::Clip));
    }

    /// A panel `Region` laid out like `aurora_ui`'s `insert_panel`: the
    /// root, an unlabelled generic first child (the title slot), a body
    /// and a controls strip (both unlabelled generic containers too).
    fn panel_tree(title: &str) -> (WidgetTree<WidgetKind>, [WidgetId; 4]) {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let mut node = accesskit::Node::new(accesskit::Role::Region);
        node.set_label(title);
        let panel = ok(tree.insert(root, taffy::Style::default(), node, WidgetKind::Panel));
        let generic = |tree: &mut WidgetTree<WidgetKind>| {
            ok(tree.insert(
                panel,
                taffy::Style::default(),
                accesskit::Node::new(accesskit::Role::GenericContainer),
                WidgetKind::Container,
            ))
        };
        let header = generic(&mut tree);
        let body = generic(&mut tree);
        let strip = generic(&mut tree);
        (tree, [panel, header, body, strip])
    }

    /// 0.142.0: a panel's first child draws the panel's label, inset like
    /// a tree row's, in `text.secondary`, ellipsized; its root, body and
    /// controls strip draw nothing.
    #[test]
    fn a_panels_title_slot_draws_the_panel_label_and_nothing_else_does() {
        let (tree, [panel, header, body, strip]) = panel_tree("Layers");
        let scales = test_scales();
        let theme = dark_theme();
        let bounds = Rect {
            x: 1350,
            y: 0,
            width: 250,
            height: 21,
        };
        let runs = |id| text_runs(&tree, id, bounds, bounds, None, &theme, &scales);
        let title_runs = runs(header);
        let [title] = &title_runs[..] else {
            unreachable!("exactly one title run: {title_runs:?}");
        };
        assert_eq!(title.text, "Layers");
        assert_eq!(title.color, rgba(theme.text.secondary, 1.0));
        assert_eq!(title.align, HAlign::Start);
        assert_eq!(title.overflow, super::TextOverflow::Ellipsis);
        assert!(title.field.is_none());
        let pad = super::token_px(scales.spacing.sm);
        assert_eq!(title.rect, (1350.0 + pad, 0.0, 250.0 - 2.0 * pad, 21.0));
        assert!(runs(panel).is_empty(), "the root draws nothing itself");
        assert!(runs(body).is_empty(), "the body is not a title slot");
        assert!(runs(strip).is_empty(), "nor is a controls strip");
    }

    /// The slot must be unlabelled and the panel's: a labelled first child
    /// or a generic first child of a non-panel draws no title, and an
    /// empty title draws nothing.
    #[test]
    fn only_a_panels_unlabelled_first_child_is_its_title_slot() {
        let scales = test_scales();
        let theme = dark_theme();
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 21,
        };
        let (mut tree, [_, header, _, _]) = panel_tree("Layers");
        let mut labelled = accesskit::Node::new(accesskit::Role::GenericContainer);
        labelled.set_label("Something");
        ok(tree.set_accessibility(header, labelled));
        assert!(text_runs(&tree, header, bounds, bounds, None, &theme, &scales).is_empty());

        let (tree, [_, header, _, _]) = panel_tree("");
        assert!(text_runs(&tree, header, bounds, bounds, None, &theme, &scales).is_empty());

        let (mut tree, root) = new_tree(taffy::Style::default());
        let mut node = accesskit::Node::new(accesskit::Role::Region);
        node.set_label("Not a panel");
        let region = ok(tree.insert(root, taffy::Style::default(), node, WidgetKind::Container));
        let first = ok(tree.insert(
            region,
            taffy::Style::default(),
            accesskit::Node::new(accesskit::Role::GenericContainer),
            WidgetKind::Container,
        ));
        assert!(text_runs(&tree, first, bounds, bounds, None, &theme, &scales).is_empty());
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
        sticky_scroll, text_runs, to_phys,
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

    /// `sticky_scroll` (0.138.0 review, critic C1): the scroll a text
    /// field keeps between frames.
    #[test]
    fn sticky_scroll_keeps_its_window_until_the_caret_leaves_it() {
        let w = CARET_WIDTH;
        // The line and an end caret fit: always 0, whatever came before.
        assert_eq!(sticky_scroll(40.0, 50.0, 50.0, 100.0), 0.0);
        assert_eq!(sticky_scroll(0.0, 100.0 - w, 99.0, 100.0), 0.0);
        // Nearly fits (RT133-01): an end caret still scrolls into view.
        assert_eq!(sticky_scroll(0.0, 100.0, 100.0, 100.0), w);
        // Caret inside the window [prev, prev + 100 - w]: unchanged, at
        // both edges and between.
        for caret in [80.0, 120.0, 180.0 - w] {
            assert_eq!(sticky_scroll(80.0, 300.0, caret, 100.0), 80.0, "{caret}");
        }
        // Past the right edge: the least scroll bringing it just inside.
        assert_eq!(sticky_scroll(80.0, 300.0, 200.0, 100.0), 200.0 + w - 100.0);
        // Past the left edge: the least scroll, caret at the left edge.
        assert_eq!(sticky_scroll(80.0, 300.0, 30.0, 100.0), 30.0);
        assert_eq!(sticky_scroll(80.0, 300.0, 0.0, 100.0), 0.0);
        // Clamped to [0, line + w - inner]: a stale scroll from a longer
        // line, or a negative one, is pulled back.
        assert_eq!(sticky_scroll(500.0, 300.0, 250.0, 100.0), 300.0 + w - 100.0);
        assert_eq!(sticky_scroll(-20.0, 300.0, 50.0, 100.0), 0.0);
        // Caret at the end of an overflowing line agrees with the pinned
        // rule (the palette query's case).
        assert_eq!(
            sticky_scroll(0.0, 300.0, 300.0, 100.0),
            field_scroll(300.0, 300.0, 100.0)
        );
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(sticky_scroll(bad, 300.0, 1.0, 100.0), 0.0);
            assert_eq!(sticky_scroll(10.0, bad, 1.0, 100.0), 0.0);
            assert_eq!(sticky_scroll(10.0, 300.0, bad, 100.0), 0.0);
            assert_eq!(sticky_scroll(10.0, 300.0, 1.0, bad), 0.0);
        }
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
            overflow: super::TextOverflow::Clip,
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
            // Caret-pinned (the palette query's rule): these tests pin
            // `resolve_run`'s drawing, not the scroll policy, which
            // `sticky_scroll`'s own tests cover.
            scroll: None,
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

    /// 0.141.0: the title slot draws the root's own label -- the title
    /// -- in `text.primary`, flush left, inside the slot's own laid-out
    /// box, and the message draws below it in its own box.
    #[test]
    fn a_dialog_draws_its_title_in_its_own_row_above_the_message() {
        let (mut tree, root) = new_tree(taffy::Style {
            size: taffy::Size {
                width: taffy::style_helpers::length(800.0_f32),
                height: taffy::style_helpers::length(600.0_f32),
            },
            ..Default::default()
        });
        let scales = test_scales();
        let theme = dark_theme();
        let handle = ok(insert_dialog(
            &mut tree,
            root,
            &scales,
            "Aurora Didn't Close Properly",
            "Something happened.",
            vec![DialogAction::new("ok", "OK")],
        ));
        tree.compute_layout(800.0, 600.0);
        let runs_in = |id| {
            let Some(bounds) = tree.bounds(id) else {
                unreachable!("laid out")
            };
            (
                bounds,
                text_runs(&tree, id, bounds, bounds, None, &theme, &scales),
            )
        };
        let (title_box, title_runs) = runs_in(handle.title);
        let [title] = &title_runs[..] else {
            unreachable!("the title draws exactly one run: {title_runs:?}");
        };
        assert_eq!(title.text, "Aurora Didn't Close Properly");
        assert_eq!(title.color, rgba(theme.text.primary, 1.0));
        assert_eq!(title.align, HAlign::Start);
        assert!(title.field.is_none());
        assert_eq!(title.rect, super::rect_f32(title_box));
        assert_eq!(title.clip, title_box);

        let (message_box, message_runs) = runs_in(handle.message);
        let [message] = &message_runs[..] else {
            unreachable!("the message still draws: {message_runs:?}");
        };
        assert_eq!(message.text, "Something happened.");
        assert!(
            message.rect.1 >= title.rect.1 + title.rect.3,
            "the message draws below the title: {message:?} vs {title:?}"
        );
        assert_eq!(message.rect, super::rect_f32(message_box));
        // The root itself still draws nothing: the title is drawn once.
        let (_, root_runs) = runs_in(handle.root);
        assert!(root_runs.is_empty(), "{root_runs:?}");
    }

    /// Only a dialog's own unlabelled `GenericContainer` child is a title
    /// slot: the same presentational node anywhere else draws nothing,
    /// and an empty title draws no run at all.
    #[test]
    fn only_a_dialogs_own_slot_draws_the_title_and_an_empty_title_draws_nothing() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        let stray = ok(tree.insert(
            root,
            taffy::Style::default(),
            accesskit::Node::new(accesskit::Role::GenericContainer),
            WidgetKind::Container,
        ));
        assert!(texts_of(&tree, stray, None).is_empty());
        let handle = ok(insert_dialog(&mut tree, root, &scales, "", "M", vec![]));
        assert!(texts_of(&tree, handle.title, None).is_empty());
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

#[cfg(test)]
// The caret's colour is compared bit-exactly: both sides are the same
// token's `to_srgb_f32`, never accumulated arithmetic.
#[allow(clippy::float_cmp)]
mod offset_tests {
    use super::{Resolved, field_offset_at, nearest_caret, resolve_run, text_runs};
    use crate::tree::{WidgetId, WidgetTree};
    use crate::widgets::{
        WidgetKind, insert_text_field, new_tree, test_scales, text_field_state, with_text_field_mut,
    };
    use aurora_core::Rect;
    use aurora_text::TextEngine;
    use aurora_theme::{Palette, Theme, ThemeSet};

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

    /// A text field holding `content`, laid out at (20, 10) 120x24.
    fn field(content: &str) -> (WidgetTree<WidgetKind>, WidgetId) {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        let rect = |x, y, width, height| Rect {
            x,
            y,
            width,
            height,
        };
        ok(tree.set_bounds(root, rect(0, 0, 400, 300)));
        let id = ok(insert_text_field(&mut tree, root, &scales, "Name", content));
        ok(tree.set_bounds(id, rect(20, 10, 120, 24)));
        (tree, id)
    }

    fn offset(
        engine: &mut TextEngine,
        tree: &WidgetTree<WidgetKind>,
        id: WidgetId,
        scale: f32,
        x: f32,
    ) -> Option<usize> {
        field_offset_at(engine, tree, id, &dark_theme(), &test_scales(), scale, x)
    }

    /// The logical x of the left edge of the caret `resolve_run` draws
    /// for `id` with its cursor at `byte` — the pixel a user clicks on.
    fn drawn_caret_x(
        engine: &mut TextEngine,
        tree: &mut WidgetTree<WidgetKind>,
        id: WidgetId,
        byte: usize,
        scale: f32,
    ) -> f32 {
        ok(with_text_field_mut(tree, id, |s| {
            s.cursor = byte;
            s.selection_anchor = None;
        }));
        frame(engine, tree, scale);
        let Some(bounds) = tree.bounds(id) else {
            unreachable!("laid out");
        };
        let theme = dark_theme();
        let Some(run) = text_runs(tree, id, bounds, bounds, Some(id), &theme, &test_scales())
            .into_iter()
            .next()
        else {
            unreachable!("a focused field draws");
        };
        let Some(caret_color) = run.field.as_ref().map(|f| f.caret_color) else {
            unreachable!("a field run carries decor");
        };
        let carets: Vec<i32> = resolve_run(engine, &run, scale)
            .into_iter()
            .filter_map(|piece| match piece {
                Resolved::Rect(r, c) if c == caret_color => Some(r[0]),
                _ => None,
            })
            .collect();
        let [x0] = carets.as_slice() else {
            unreachable!("exactly one caret, got {carets:?}");
        };
        #[allow(clippy::cast_precision_loss)]
        let x = *x0 as f32 / scale;
        x
    }

    const SHORT: &str = "Hello, world";
    const LONG: &str = "The quick brown fox jumps over the lazy dog, twice over";

    /// Every caret `resolve_run` draws maps back to its own byte — with
    /// the field scrolled to show it (an overflowing line) or not — and
    /// so does a point a quarter px left of it, which a floor-style
    /// "caret at or before the point" rule would give to the previous
    /// byte.
    #[test]
    fn every_drawn_caret_maps_back_to_its_byte_at_scale_one_and_two() {
        let mut engine = ok(TextEngine::new());
        for content in [SHORT, LONG] {
            for scale in [1.0_f32, 2.0] {
                let (mut tree, id) = field(content);
                let mut scrolled = false;
                for byte in 0..=content.len() {
                    let x = drawn_caret_x(&mut engine, &mut tree, id, byte, scale);
                    // The caret sits where it would unscrolled only while
                    // the line fits; past the box's width it is scrolled.
                    let line = engine.shape(content, &super::label_style(&test_scales()), scale);
                    // The field's box starts at x = 20, its text `spacing.sm` in.
                    let inner = 20.0 + super::token_px(test_scales().spacing.sm);
                    let unscrolled = inner + line.caret_x(byte).unwrap_or(0.0);
                    scrolled |= x + 1.0 < unscrolled;
                    for probe in [x + 0.25 / scale, x - 0.25] {
                        assert_eq!(
                            offset(&mut engine, &tree, id, scale, probe),
                            Some(byte),
                            "{content:?} at scale {scale}: byte {byte}, probe {probe}"
                        );
                    }
                }
                assert_eq!(
                    scrolled,
                    content == LONG,
                    "{content:?} at {scale}: only the overflowing line scrolls"
                );
            }
        }
    }

    /// A point past either end — or in the field's padding — clamps to
    /// the padded text box before the nearest caret is picked (0.138.0
    /// review, critic C2). For a line that fits, or one scrolled to that
    /// end, that is `0` or the content's length (0.138.0's original
    /// contract, kept); for an end scrolled out of view it is the
    /// nearest *visible* caret, which placing the caret there leaves
    /// visible (the stored scroll does not move). Before C2 a press in
    /// the padding of a scrolled field could pick a caret scrolled out
    /// of view: at scale 1, `LONG` with its caret at `0`, a point far
    /// right picked byte 55 (the end), with 14 the last visible caret.
    #[test]
    fn a_point_past_either_end_clamps_to_the_nearest_visible_caret() {
        let mut engine = ok(TextEngine::new());
        let pad = super::token_px(test_scales().spacing.sm);
        let (inner_left, inner_right) = (20.0 + pad, 20.0 + 120.0 - pad);
        for content in [SHORT, LONG] {
            for scale in [1.0_f32, 2.0] {
                let (mut tree, id) = field(content);
                for cursor in [content.len(), 0] {
                    ok(with_text_field_mut(&mut tree, id, |s| s.cursor = cursor));
                    frame(&mut engine, &mut tree, scale);
                    let scroll = ok(text_field_state(&tree, id)).scroll();
                    let first = offset(&mut engine, &tree, id, scale, inner_left);
                    let last = offset(
                        &mut engine,
                        &tree,
                        id,
                        scale,
                        inner_right - super::CARET_WIDTH,
                    );
                    let at = |engine: &mut TextEngine, x| offset(engine, &tree, id, scale, x);
                    let case = format!("{content:?} at {scale}, cursor {cursor}");
                    assert_eq!(at(&mut engine, -1000.0), first, "{case}");
                    assert_eq!(at(&mut engine, 20.5), first, "{case}: left padding");
                    assert_eq!(at(&mut engine, 10_000.0), last, "{case}");
                    assert_eq!(at(&mut engine, 139.5), last, "{case}: right padding");
                    let (Some(first), Some(last)) = (first, last) else {
                        unreachable!("{case}: a laid-out field answers");
                    };
                    match (content == LONG, cursor == 0) {
                        (false, _) => assert_eq!((first, last), (0, content.len()), "{case}"),
                        (true, true) => {
                            assert_eq!(first, 0, "{case}");
                            assert!(last < content.len(), "{case}: the end is scrolled away");
                        }
                        (true, false) => {
                            assert!(first > 0, "{case}: the start is scrolled away");
                            assert_eq!(last, content.len(), "{case}");
                        }
                    }
                    // Each pick is a visible caret: placing it there does
                    // not move the stored scroll.
                    for byte in [first, last] {
                        ok(with_text_field_mut(&mut tree, id, |s| s.cursor = byte));
                        frame(&mut engine, &mut tree, scale);
                        assert_eq!(
                            ok(text_field_state(&tree, id)).scroll(),
                            scroll,
                            "{case}: byte {byte} was not visible"
                        );
                    }
                    ok(with_text_field_mut(&mut tree, id, |s| s.cursor = cursor));
                }
            }
        }
    }

    #[test]
    fn nearest_caret_breaks_an_exact_tie_toward_the_lower_byte() {
        let carets = [(0, 0.0), (1, 10.0), (4, 20.0)];
        assert_eq!(nearest_caret(&carets, 5.0), Some(0));
        assert_eq!(nearest_caret(&carets, 5.01), Some(1));
        assert_eq!(nearest_caret(&carets, 15.0), Some(1));
        assert_eq!(nearest_caret(&carets, 15.01), Some(4));
        assert_eq!(nearest_caret(&carets, -3.0), Some(0));
        assert_eq!(nearest_caret(&carets, 99.0), Some(4));
        assert_eq!(nearest_caret(&carets, f32::NAN), None);
        assert_eq!(nearest_caret(&[], 1.0), None);
    }

    #[test]
    fn no_offset_while_composing_or_for_another_widget() {
        let mut engine = ok(TextEngine::new());
        let (mut tree, id) = field(SHORT);
        assert!(offset(&mut engine, &tree, id, 1.0, 40.0).is_some());
        ok(with_text_field_mut(&mut tree, id, |s| {
            s.set_composition("ni", None);
        }));
        assert_eq!(offset(&mut engine, &tree, id, 1.0, 40.0), None);
        let root = tree.root();
        assert_eq!(offset(&mut engine, &tree, root, 1.0, 40.0), None);
    }

    /// The real hit ([`field_offset_at`]) the app builds, at `scale`.
    struct Hit<'a> {
        engine: &'a mut TextEngine,
        scale: f32,
    }

    impl crate::TextHit for Hit<'_> {
        fn offset_at(
            &mut self,
            tree: &WidgetTree<WidgetKind>,
            id: WidgetId,
            point: (f32, f32),
        ) -> Option<usize> {
            offset(self.engine, tree, id, self.scale, point.0)
        }
    }

    /// One frame's scroll update, as the app runs it before painting.
    fn frame(engine: &mut TextEngine, tree: &mut WidgetTree<WidgetKind>, scale: f32) {
        super::update_field_scrolls(engine, tree, &dark_theme(), &test_scales(), scale);
    }

    /// Critic C1 (0.138.0 review): in an overflowing field, a click and
    /// a drag held still must leave the text where it is under the
    /// pointer. With the old caret-pinned scroll every `Move` re-scrolled
    /// from the caret the previous `Move` set, so the selection ran away
    /// leftward and a plain click shifted the text under the pointer.
    #[test]
    fn a_click_and_a_still_drag_in_an_overflowing_field_stay_under_the_pointer() {
        use crate::{ClickTracker, FocusManager, PointerEvent, PointerPhase};
        let mut engine = ok(TextEngine::new());
        for scale in [1.0_f32, 2.0] {
            let (mut tree, id) = field(LONG);
            let mut focus = FocusManager::new();
            let mut click = ClickTracker::default();
            // Caret at the end: the field is scrolled to its far right.
            frame(&mut engine, &mut tree, scale);
            let x = 20.0 + 60.0;
            let y = 22.0;
            let before = offset(&mut engine, &tree, id, scale, x);
            let scroll = ok(text_field_state(&tree, id)).scroll();
            assert!(scroll > 0.0, "scale {scale}: the line overflows");
            assert!(
                before.is_some_and(|b| b > 0 && b < LONG.len()),
                "{before:?}"
            );
            let mut event = |tree: &mut WidgetTree<WidgetKind>, engine: &mut TextEngine, phase| {
                ok(crate::handle_pointer_with(
                    tree,
                    &mut focus,
                    &mut click,
                    PointerEvent {
                        phase,
                        position: (x, y),
                    },
                    crate::shortcut::Modifiers::none(),
                    &mut Hit { engine, scale },
                ))
            };
            event(&mut tree, &mut engine, PointerPhase::Down);
            let state = |tree: &WidgetTree<WidgetKind>| {
                let s = ok(text_field_state(tree, id));
                (s.cursor, s.selection_anchor)
            };
            assert_eq!(state(&tree), (ok_some(before), None), "scale {scale}");
            for step in 0..6 {
                frame(&mut engine, &mut tree, scale);
                // What is under the pointer did not move.
                assert_eq!(
                    offset(&mut engine, &tree, id, scale, x),
                    before,
                    "scale {scale}, frame {step}: the text jumped under the pointer"
                );
                assert_eq!(
                    ok(text_field_state(&tree, id)).scroll(),
                    scroll,
                    "scale {scale}, frame {step}: a click in the window keeps the scroll"
                );
                event(&mut tree, &mut engine, PointerPhase::Move);
                assert_eq!(
                    state(&tree),
                    (ok_some(before), None),
                    "scale {scale}, move {step}: a still drag selected nothing"
                );
            }
        }
    }

    fn ok_some(value: Option<usize>) -> usize {
        value.unwrap_or(usize::MAX)
    }
}
