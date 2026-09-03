//! Resolves a widget's own layout bounds and state into the geometry
//! and colour [`crate::render::PathPipeline`]/[`crate::render::GpuMesh`]
//! need to actually draw it — the "wiring a widget's own paint through
//! this pipeline" step `render`'s own doc comment names as still open.
//!
//! **Scope, stated honestly.** [`paint_widget`] covers `Button`,
//! `Checkbox`, `Slider`, `Scrollbar`, `TextField`, `CommandPalette`,
//! `ColorSwatch`,
//! `ListRow`, `TreeItem`, `Panel`, and `Dialog` — solid rounded-rect shapes, the simplest of
//! the widgets this crate has (`widgets`' own doc comment). `Checkbox`'s
//! own box has no check/dash
//! *glyph* drawn inside it yet (this crate draws no glyphs at all —
//! solid fills only, `render`'s own doc comment); `Toggled::True` and
//! `Toggled::Mixed` currently render identically (both
//! `accent.primary`) since nothing yet exists to tell them apart
//! visually. `TextField` paints its own background only — no caret, no
//! selection highlight, no composition underline
//! (`composition_segments`' own byte-range *data* has no pixel
//! position to map to without real text shaping, which doesn't exist
//! in this crate; `content`/`cursor`/`selection_anchor`/`composition`
//! don't affect its paint at all today, only `disabled` does).
//! `CommandPalette` paints its own outer panel only — its query field's
//! own text still isn't drawn (the same "no real text shaping yet" gap
//! `TextField` has). Its result rows now are painted, though: each is a
//! real `WidgetKind::ListRow` (`command_palette::rebuild_rows`), and
//! [`paint_list_row`] highlights the selected one with `accent.primary`
//! — an unselected row still paints nothing, the same "nothing to
//! highlight" `Ok(vec![])` every other unselected-state widget already
//! returns. `Scrollbar` paints a full-length track and a proportional
//! thumb on top of it, but that is *all* it is: nothing in this crate
//! scrolls any content, so the thumb's own position and length are a
//! pure function of its state's numbers and never of a real viewport
//! (`widgets::scrollbar`'s own module doc comment). `TreeItem` paints
//! the same selection highlight `ListRow` does, from the same token,
//! with one real difference: a row's own layout box grows to contain
//! its children, so the fill is clamped to one row's height
//! ([`paint_tree_item`]) — a selected group would otherwise paint over
//! every descendant beneath it. It draws no disclosure triangle and no
//! label (this crate draws no glyphs at all), so a collapsed row and an
//! expanded one are pixel-identical apart from what their descendants
//! do; `expanded` reaches the accessibility node only. `Dialog` paints
//! a modal's own surface — a `surface.overlay` rounded rect with an
//! unconditional `border.default` outline, the same fill-plus-border
//! shape `Panel` already has and for the same measured reason (without
//! it, a Light-theme dialog is byte-identical to the panel behind it;
//! [`paint_dialog`] has the full account, the vocabulary citation for
//! `surface.overlay` over `surface.raised`, and the honest
//! Colour-Critical residual) — and **nothing else**: no title glyph, no
//! message glyph (this crate draws no glyphs), and no scrim dimming the
//! window behind it (out of scope, see `widgets::dialog`'s own module
//! doc comment). Every other
//! [`WidgetKind`] (`Container` on its own, a dialog's own message node
//! included) returns `Ok(vec![])` too — a real, deliberate "nothing to
//! paint," not an error.
//!
//! Every kind's own geometry is built from bounds that
//! [`clip_to_clipping_ancestors`] has already intersected with any
//! ancestor declaring a clipping `taffy::Overflow`, so no widget paints
//! outside the panel that holds it and one entirely past its panel's
//! edge paints nothing at all — see that function for the measured case
//! (a 21 px row in a 13 px panel body) and for why no per-widget height
//! clamp can stand in for it.
//!
//! [`paint_widget`] returns a `Vec<Paint>`, not a single `Paint` —
//! `Button`/`Checkbox` only ever needed one shape, but `Slider` is the
//! first widget that genuinely needs more than one (a track *and* a
//! thumb, different geometry, different colour, drawn in that order so
//! the thumb lands on top). Widening the return type when `Slider`
//! needed it, while there were only two real call sites
//! (`aurora-app::collect_widget_paints`,
//! `tests/gallery.rs::collect_gallery_paints`) to update, was cheaper
//! than doing it later after more of either existed.
//!
//! Colour always comes from a real, resolved [`Theme`] token
//! (`accent.primary`/`accent.primary_active`, `surface.sunken`,
//! `state.disabled_opacity`) — invariant §7.3.10, never a literal. The
//! returned `[f32; 4]` is straight (unpremultiplied) sRGB-gamma-encoded
//! RGBA, [`Color::to_srgb_f32`]'s own convention — matching what
//! [`crate::render::PathPipeline::bind_group`]'s own doc comment
//! expects. This function itself never linearizes for an sRGB-aware
//! render target; that's a real caller's own job once it actually owns
//! one (`aurora-app::linearize_paint_color` does it for the real
//! swapchain, the headless gallery harness's own `render_gallery`
//! deliberately doesn't need to for its non-sRGB offscreen target) —
//! this function's job stops at "the token's own colour, resolved."
//!
//! No per-widget corner-radius token exists in
//! `design/tokens/vocabulary.md` yet (only the bare `radius.*` scale
//! does) — `scales.radius.sm`/`scales.radius.pill`/`scales.radius.md`
//! (the last for `CommandPalette`'s own larger floating panel — bigger
//! surfaces reading as more rounded is a common, but not `vocabulary.md`
//! -mandated, convention) are this function's own reasonable choices,
//! not a design decision made by Cahya (PRD FR-027 *Ownership*); revisit
//! if/when real per-widget radius tokens are added.

use accesskit::{Orientation, Toggled};
use aurora_core::Rect;
use aurora_theme::{Scales, Theme};
use aurora_vector::{Mesh, Path, fill, rounded_rect, stroke, tolerance_for_scale_factor};

use taffy::Overflow;

use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};
use crate::widgets::{
    ButtonState, CheckboxState, ColorSwatchState, ListRowState, ScrollbarState, SliderState,
    TextFieldState, TreeItemState, WidgetKind, row_height,
};

/// One shape's own paint: tessellated fill geometry plus the straight,
/// unpremultiplied RGBA colour to draw it with — exactly the pair
/// [`crate::render::PathPipeline::bind_group`]/[`crate::render::
/// GpuMesh::upload`] need. A widget's *whole* paint is a `Vec<Paint>`
/// ([`paint_widget`]'s own return type) — see this module's own doc
/// comment for why a single widget can need more than one.
pub type Paint = (Mesh, [f32; 4]);

/// The mandatory control-outline stroke `border.control`/
/// `border.control_opacity` describe (`design/tokens/vocabulary.md`) —
/// `None` when the opacity is exactly `0.0` (every theme except the two
/// High Contrast ones, not yet landed), so a widget's shape count and
/// every existing test that depends on it are completely unaffected
/// until an HC theme actually sets the opacity above zero. This is the
/// same "conditional, not padded" idiom [`paint_list_row`] already uses
/// for an unselected row's own `Ok(vec![])`, not a new pattern invented
/// here. `alpha` lets the caller fold in `state.disabled_opacity` too —
/// a disabled control's outline should dim along with everything else
/// about it, the same as its fill already does. `scale_factor` is the
/// window's own DPI scale factor (`winit::window::Window::scale_factor`)
/// — see [`aurora_vector::tolerance_for_scale_factor`] for why this
/// stroke's tolerance depends on it.
fn control_outline(
    path: &Path,
    theme: &Theme,
    alpha: f32,
    scale_factor: f32,
) -> Result<Option<Paint>, WidgetError> {
    const CONTROL_BORDER_WIDTH: f32 = 1.0;

    if theme.border.control_opacity <= 0.0 {
        return Ok(None);
    }
    let tolerance = tolerance_for_scale_factor(scale_factor);
    let mesh = stroke(path, CONTROL_BORDER_WIDTH, tolerance).map_err(WidgetError::Paint)?;
    let [r, g, b] = theme.border.control.to_srgb_f32();
    Ok(Some((
        mesh,
        [r, g, b, theme.border.control_opacity * alpha],
    )))
}

/// Resolves `id`'s own paint from its current layout bounds
/// ([`WidgetTree::bounds`]) and state — zero or more shapes, in the
/// order they should draw (later entries on top of earlier ones). An
/// empty `Vec` is a real, deliberate "nothing to paint yet" for this
/// [`WidgetKind`] (see this module's own doc comment for exactly which
/// do), not an error.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist in
/// `tree`, or [`WidgetError::Paint`] if tessellation itself fails (in
/// practice unreachable for `rounded_rect`'s own output — see
/// `WidgetError::Paint`'s own doc comment).
///
/// `scale_factor` is the window's own DPI scale factor
/// (`winit::window::Window::scale_factor`, e.g. `2.0` on a Retina/HiDPI
/// display) — threaded down into every fill/stroke call so tessellation
/// tolerance tracks physical, not just logical, pixel density (see
/// [`aurora_vector::tolerance_for_scale_factor`]). A headless caller
/// with no real window (e.g. the component-gallery test harness) should
/// pass `1.0`.
pub fn paint_widget(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> Result<Vec<Paint>, WidgetError> {
    let bounds = tree.bounds(id).ok_or(WidgetError::UnknownWidget(id))?;
    let kind = tree.payload(id).ok_or(WidgetError::UnknownWidget(id))?;
    let Some(bounds) = clip_to_clipping_ancestors(tree, id, bounds) else {
        return Ok(vec![]);
    };
    match kind {
        WidgetKind::Button(state) => paint_button(state, bounds, theme, scales, scale_factor),
        WidgetKind::Checkbox(state) => paint_checkbox(state, bounds, theme, scales, scale_factor),
        WidgetKind::Slider(state) => paint_slider(state, bounds, theme, scales, scale_factor),
        WidgetKind::Scrollbar(state) => paint_scrollbar(state, bounds, theme, scales, scale_factor),
        WidgetKind::TextField(state) => {
            paint_text_field(state, bounds, theme, scales, scale_factor)
        }
        WidgetKind::CommandPalette(_) => paint_command_palette(bounds, theme, scales, scale_factor),
        WidgetKind::ColorSwatch(state) => {
            paint_color_swatch(*state, bounds, theme, scales, scale_factor)
        }
        WidgetKind::ListRow(state) => paint_list_row(*state, bounds, theme, scales, scale_factor),
        // By reference, unlike `ListRow`/`ColorSwatch` above:
        // `TreeItemState` owns a `String` label, so it is deliberately
        // not `Copy` (see `list_row`'s own doc comment for why the two
        // row types stayed separate).
        WidgetKind::TreeItem(state) => paint_tree_item(state, bounds, theme, scales, scale_factor),
        WidgetKind::Panel => paint_panel(bounds, theme, scales, scale_factor),
        WidgetKind::Dialog => paint_dialog(bounds, theme, scales, scale_factor),
        WidgetKind::Container => Ok(vec![]),
    }
}

/// `bounds`, intersected with the box of every ancestor that clips its
/// own content — `taffy::Overflow` anything but `Visible`, tested per
/// axis, since `taffy` carries `overflow.x` and `overflow.y`
/// independently. `None` when nothing of the widget survives the
/// intersection, which [`paint_widget`] turns into the same real,
/// deliberate `Ok(vec![])` an unselected row already returns.
///
/// **This is what keeps a widget from painting outside the panel that
/// contains it, and it is a real, measured gap, not a hypothetical.**
/// A panel body (`aurora_ui::panel`'s own `body_style`, the only
/// `Overflow::Hidden` in the workspace today) gets a content-independent
/// share of the dock rail, while the rows inside it each carry a hard
/// one-line `min_size.height` floor. Measured in a real
/// `aurora_ui::build_workspace` at an 800×40 window: the History body
/// resolves to 13 px tall and its first row to 21 px, so the row's own
/// box extends 8 px past the body — and the Layers panel's tree rows do
/// exactly the same thing, at exactly the same numbers. Painting a
/// selected row's `accent.primary` fill from its own unclipped bounds
/// would lay that overhang across whatever is docked below.
///
/// `paint_tree_item`'s own `row_height(scales).min(bounds.height)` does
/// **not** cover this and never did: for a 21 px row it computes
/// `min(21, 21) = 21`. That clamp exists for a different problem — a
/// selected *group*'s box spanning its whole subtree — and this one is
/// about the ancestor, which no per-widget height clamp can see.
///
/// The clip is applied to the *rect*, before tessellation, rather than
/// as a real scissor: a partly-clipped rounded rect therefore keeps its
/// `scales.radius.sm` corners at the cut instead of being sliced flat.
/// That is a visible approximation only in the already-degenerate case
/// this exists to contain, and a genuine scissor belongs to
/// `crate::render`/the caller's own render pass, not here. Clipping to
/// nothing also makes paint agree with `WidgetTree::hit_test`, which
/// already refuses to descend into a parent whose bounds exclude the
/// point: a row fully past the bottom of its panel is now both
/// unreachable *and* invisible, rather than unreachable but drawn.
fn clip_to_clipping_ancestors(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
    bounds: Rect,
) -> Option<Rect> {
    let mut left = bounds.x;
    let mut top = bounds.y;
    let mut right = bounds.x.saturating_add(i64::from(bounds.width));
    let mut bottom = bounds.y.saturating_add(i64::from(bounds.height));
    let mut clipped = false;
    let mut current = tree.parent(id);
    while let Some(ancestor) = current {
        if let (Some(style), Some(clip)) = (tree.style(ancestor), tree.bounds(ancestor)) {
            if style.overflow.x != Overflow::Visible {
                clipped = true;
                left = left.max(clip.x);
                right = right.min(clip.x.saturating_add(i64::from(clip.width)));
            }
            if style.overflow.y != Overflow::Visible {
                clipped = true;
                top = top.max(clip.y);
                bottom = bottom.min(clip.y.saturating_add(i64::from(clip.height)));
            }
        }
        current = tree.parent(ancestor);
    }
    // Returned untouched, not merely unchanged, when no ancestor clips
    // at all: a widget whose bounds are still the default zero rect
    // (every test in this module that paints without a
    // `compute_layout`/`set_bounds` first) must keep painting the
    // degenerate shape it always did, rather than being turned into an
    // `Ok(vec![])` by an empty intersection with itself.
    if !clipped {
        return Some(bounds);
    }
    if right <= left || bottom <= top {
        return None;
    }
    Some(Rect {
        x: left,
        y: top,
        width: u32::try_from(right - left).ok()?,
        height: u32::try_from(bottom - top).ok()?,
    })
}

fn paint_button(
    state: &ButtonState,
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> Result<Vec<Paint>, WidgetError> {
    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.sm as f32,
    );
    let tolerance = tolerance_for_scale_factor(scale_factor);
    let mesh = fill(&path, tolerance).map_err(WidgetError::Paint)?;

    // No hover flag exists on `ButtonState` yet (`widgets::button`'s own
    // doc comment), so only the two states it actually tracks are
    // resolved here -- `accent.primary_hover` stays unused until a real
    // pointer-hover concept reaches this state, not applied speculatively.
    let base = if state.pressed {
        theme.accent.primary_active
    } else {
        theme.accent.primary
    };
    let [r, g, b] = base.to_srgb_f32();
    let alpha = if state.disabled {
        theme.state.disabled_opacity
    } else {
        1.0
    };
    let mut paints = vec![(mesh, [r, g, b, alpha])];
    if let Some(outline) = control_outline(&path, theme, alpha, scale_factor)? {
        paints.push(outline);
    }
    Ok(paints)
}

fn paint_checkbox(
    state: &CheckboxState,
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> Result<Vec<Paint>, WidgetError> {
    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.sm as f32,
    );
    let tolerance = tolerance_for_scale_factor(scale_factor);
    let mesh = fill(&path, tolerance).map_err(WidgetError::Paint)?;

    // `Toggled::True`/`Toggled::Mixed` share a colour -- see this
    // module's own doc comment for why (no check/dash glyph exists yet
    // to actually tell them apart).
    let base = match state.checked {
        Toggled::True | Toggled::Mixed => theme.accent.primary,
        Toggled::False => theme.surface.sunken,
    };
    let [r, g, b] = base.to_srgb_f32();
    let alpha = if state.disabled {
        theme.state.disabled_opacity
    } else {
        1.0
    };
    let mut paints = vec![(mesh, [r, g, b, alpha])];
    if let Some(outline) = control_outline(&path, theme, alpha, scale_factor)? {
        paints.push(outline);
    }
    Ok(paints)
}

/// `Slider`'s own two shapes, in draw order: a track (a thin, full-width
/// pill-shaped bar, `surface.sunken` — the same "recessed input
/// control" token `Checkbox`'s own unchecked box already uses) and a
/// thumb on top of it (a circular knob — `scales.radius.pill`'s own
/// 9999 clamps down to a real circle against any shape this small, the
/// same reasoning that already applies to the track's own rounded
/// ends), positioned at `state.value`'s own proportional offset along
/// `state.min..=state.max`. `disabled_opacity` is applied to both
/// shapes uniformly, not just one.
fn paint_slider(
    state: &SliderState,
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> Result<Vec<Paint>, WidgetError> {
    let alpha = if state.disabled {
        theme.state.disabled_opacity
    } else {
        1.0
    };
    let tolerance = tolerance_for_scale_factor(scale_factor);

    let track_thickness = bounds.height as f32 * 0.3;
    let track_path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32 + (bounds.height as f32 - track_thickness) / 2.0,
        bounds.width as f32,
        track_thickness,
        scales.radius.pill as f32,
    );
    let track_mesh = fill(&track_path, tolerance).map_err(WidgetError::Paint)?;
    let [r, g, b] = theme.surface.sunken.to_srgb_f32();
    let track = (track_mesh, [r, g, b, alpha]);

    // `range <= 0.0` is a degenerate slider (`min == max`, or a caller
    // that ignored `insert_slider`'s own "assumes min <= max"
    // documented precondition) -- parked at the track's own left edge
    // rather than dividing by zero/producing a NaN position.
    let range = state.max - state.min;
    let fraction = if range > 0.0 {
        ((state.value - state.min) / range).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let thumb_size = bounds.height as f32;
    let thumb_travel = (bounds.width as f32 - thumb_size).max(0.0);
    let thumb_path = rounded_rect(
        bounds.x as f32 + fraction as f32 * thumb_travel,
        bounds.y as f32,
        thumb_size,
        thumb_size,
        scales.radius.pill as f32,
    );
    let thumb_mesh = fill(&thumb_path, tolerance).map_err(WidgetError::Paint)?;
    let [r, g, b] = theme.accent.primary.to_srgb_f32();
    let thumb = (thumb_mesh, [r, g, b, alpha]);

    let mut paints = vec![track, thumb];
    // The thumb, not the track: the track is a groove, not itself a
    // focusable control -- the thumb is the actual interactive handle a
    // user grabs (this module's own doc comment / `control_outline`'s).
    if let Some(outline) = control_outline(&thumb_path, theme, alpha, scale_factor)? {
        paints.push(outline);
    }
    Ok(paints)
}

/// `value` if it is finite, `fallback` otherwise — the one-line guard
/// that keeps a `NaN`/infinite fraction from reaching a tessellator.
/// Written out rather than leaned on `f32::max`'s own NaN-laundering
/// (which does silently replace a `NaN` operand) because that only
/// happens to cover *some* of the arithmetic downstream, and relying on
/// it would leave the rest — an ordinary multiply — still producing
/// `NaN`. See [`paint_scrollbar`]'s own doc comment.
fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

/// `Scrollbar`'s own two shapes, in draw order: a full-length track
/// (`surface.sunken`, the same "recessed control" token a `Slider`'s own
/// track already uses) and a thumb on top of it (`accent.primary`), both
/// `scales.radius.pill`. `disabled_opacity` is applied to both shapes
/// uniformly, exactly as [`paint_slider`] already does.
///
/// The one real difference from a slider: a scrollbar's thumb has a
/// *length* of its own, proportional to how much of the scrolled content
/// is visible (`state.page_size` against the whole scrollable span), not
/// a fixed square knob. It is floored at the bar's own cross-axis
/// thickness so a huge document still leaves something grabbable, and
/// capped at the track's own length so it can never overhang its track
/// (which a very short, thick bar would otherwise do).
///
/// Every degenerate input is handled before any division: a zero or
/// negative span (`min == max` with no page) paints a full-length thumb
/// parked at the track's own start, rather than dividing by zero and
/// tessellating a NaN rectangle.
///
/// **Guarding the divisor is not enough, and this function learned that
/// the hard way.** `span > 0.0`/`range > 0.0` only rule out a division
/// *by* zero; they say nothing about the quotient. `min =
/// f64::NEG_INFINITY` with `max = f64::INFINITY` satisfies every bound
/// check there is (it is finite-free but perfectly ordered), yet makes
/// `range` infinite and `(value - min) / range` an honest `inf / inf =
/// NaN`; a `NaN` `state.value` does the same with an ordinary range,
/// since `f64::clamp` propagates `NaN` rather than clamping it. Either
/// one reaches `lyon` as a `NaN` rectangle, which trips its own
/// `assert!(p.y.is_finite())` in a debug build and returns
/// `Err(WidgetError::Paint)` in a release one — a build-profile-
/// dependent panic in a crate that denies panics. So both *fractions*
/// are forced finite below, after the division, and this function is
/// total for every `ScrollbarState` that can be constructed, including
/// one reached through the public `WidgetTree::payload_mut`.
fn paint_scrollbar(
    state: &ScrollbarState,
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> Result<Vec<Paint>, WidgetError> {
    let alpha = if state.disabled {
        theme.state.disabled_opacity
    } else {
        1.0
    };
    let tolerance = tolerance_for_scale_factor(scale_factor);
    let radius = scales.radius.pill as f32;

    let left = bounds.x as f32;
    let top = bounds.y as f32;
    let width = bounds.width as f32;
    let height = bounds.height as f32;
    let vertical = matches!(state.orientation, Orientation::Vertical);
    let (track_len, thickness) = if vertical {
        (height, width)
    } else {
        (width, height)
    };

    let track_path = rounded_rect(left, top, width, height, radius);
    let track_mesh = fill(&track_path, tolerance).map_err(WidgetError::Paint)?;
    let [r, g, b] = theme.surface.sunken.to_srgb_f32();
    let track = (track_mesh, [r, g, b, alpha]);

    // The whole scrollable extent is the travel *plus* one page -- a bar
    // whose page covers the entire content (`max == min`) is a
    // full-length thumb, not a zero-length one.
    let span = (state.max - state.min) + state.page_size;
    let thumb_fraction = if span > 0.0 {
        (state.page_size / span).clamp(0.0, 1.0)
    } else {
        1.0
    };
    // A non-finite quotient falls back to the same "no proportional
    // information" answer a zero span already gives: a full-length
    // thumb. See this function's own doc comment.
    let thumb_fraction = finite_or(thumb_fraction, 1.0);
    let thumb_len = (track_len * thumb_fraction as f32)
        .max(thickness)
        .min(track_len);

    let range = state.max - state.min;
    let position_fraction = if range > 0.0 {
        ((state.value - state.min) / range).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // ... and a non-finite position falls back to the track's own
    // start, the same answer a zero range already gives.
    let position_fraction = finite_or(position_fraction, 0.0);
    let offset = position_fraction as f32 * (track_len - thumb_len).max(0.0);

    let (thumb_left, thumb_top, thumb_width, thumb_height) = if vertical {
        (left, top + offset, thickness, thumb_len)
    } else {
        (left + offset, top, thumb_len, thickness)
    };
    let thumb_path = rounded_rect(thumb_left, thumb_top, thumb_width, thumb_height, radius);
    let thumb_mesh = fill(&thumb_path, tolerance).map_err(WidgetError::Paint)?;
    let [r, g, b] = theme.accent.primary.to_srgb_f32();
    let thumb = (thumb_mesh, [r, g, b, alpha]);

    let mut paints = vec![track, thumb];
    // The thumb, not the track -- the same reasoning `paint_slider`
    // already records: the track is a groove, the thumb is the handle.
    if let Some(outline) = control_outline(&thumb_path, theme, alpha, scale_factor)? {
        paints.push(outline);
    }
    Ok(paints)
}

fn paint_text_field(
    state: &TextFieldState,
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> Result<Vec<Paint>, WidgetError> {
    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.sm as f32,
    );
    let tolerance = tolerance_for_scale_factor(scale_factor);
    let mesh = fill(&path, tolerance).map_err(WidgetError::Paint)?;

    // The same "recessed input control" token an unchecked Checkbox and
    // a Slider's own track already use -- `content`/`cursor`/
    // `selection_anchor`/`composition` don't affect this at all, see
    // this module's own doc comment for why.
    let [r, g, b] = theme.surface.sunken.to_srgb_f32();
    let alpha = if state.disabled {
        theme.state.disabled_opacity
    } else {
        1.0
    };
    let mut paints = vec![(mesh, [r, g, b, alpha])];
    if let Some(outline) = control_outline(&path, theme, alpha, scale_factor)? {
        paints.push(outline);
    }
    Ok(paints)
}

/// `CommandPalette`'s own outer panel, nothing else — `paint_list_row`
/// covers the selected-row highlight separately, since each row now
/// paints itself (`WidgetKind::ListRow`, dispatched independently by
/// `paint_widget` when `WidgetTree::paint_order` reaches it, not drawn
/// by this function). `scales.radius.md` (a floating panel reading as
/// more rounded than a small control is a common convention, not
/// `scales.radius.sm` — see this module's own doc comment).
/// `surface.raised`, not `surface.overlay`: `design/tokens/vocabulary.md`
/// defines `surface.raised` as "Elevation 1: dropdowns, popovers,
/// context menus" and reserves `surface.overlay` for "Elevation 2:
/// modals, dialogs" — a command palette is the former, a floating,
/// dismissable popover, not a blocking modal.
///
/// Still a real, honest gap: the query field's own text isn't drawn
/// (no text shaping in this crate yet, the same gap `TextField` has).
fn paint_command_palette(
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> Result<Vec<Paint>, WidgetError> {
    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.md as f32,
    );
    let tolerance = tolerance_for_scale_factor(scale_factor);
    let mesh = fill(&path, tolerance).map_err(WidgetError::Paint)?;
    let [r, g, b] = theme.surface.raised.to_srgb_f32();
    let mut paints = vec![(mesh, [r, g, b, 1.0])];
    if let Some(outline) = control_outline(&path, theme, 1.0, scale_factor)? {
        paints.push(outline);
    }
    Ok(paints)
}

/// A modal dialog's own surface: a `scales.radius.md` rounded rect
/// filled with `surface.overlay`, an **unconditional `border.default`
/// outline over it**, and the conditional [`control_outline`] on top of
/// that in a theme whose `border.control_opacity` is above zero. No
/// title glyph, no message glyph, no scrim.
///
/// **`surface.overlay`, not `surface.raised`.**
/// `design/tokens/vocabulary.md` defines `surface.overlay` as
/// "Elevation 2: modals, dialogs" and `surface.raised` as "Elevation 1:
/// dropdowns, popovers, context menus" — a modal alert
/// (`widgets::dialog` builds nothing else: `Role::AlertDialog` plus
/// `Node::set_modal`) is the former, a command palette the latter.
///
/// **The unconditional border is the load-bearing part, not decoration
/// — a fill alone made this widget genuinely invisible.** In the Light
/// theme `design/themes/light.toml` resolves `surface.overlay`,
/// `surface.raised`, `surface.panel` *and* `surface.canvas` all to
/// `neutral.900` `#f5f5f6`, and sets `border.control_opacity = 0.0`, so
/// through `0.79.0` a dialog's entire paint was one `#f5f5f6` fill with
/// [`control_outline`] returning `None` — byte-identical to the
/// [`paint_panel`] surface behind it, at 1.000:1, with nothing else
/// (this crate draws no shadows) to separate them. That is reachable in
/// the shipping app: `aurora-app`'s enforced 640×480 minimum window
/// still centres a real dialog over real `WidgetKind::Panel` chrome.
/// [`paint_panel`] hit exactly this failure mode once already, on real
/// hardware, and was fixed with an unconditional `border.default`
/// stroke at a 1.0 logical-pixel width; this is that same fix, the same
/// token, the same width, applied to the same class of bug rather than
/// a new invention. In Light it buys a 2.47:1 edge against both the
/// dialog's own fill and the panel behind it.
///
/// **The honest residual: Colour-Critical.** There, `border.default`
/// (`cc.border_mid` `#6e6e6e`) clears `cc.overlay` `#5a5a5a` by only
/// ≈1.35:1 and `cc.canvas` `#545454` by ≈1.49:1 — a real edge, but a
/// faint one. That is **not a new gap and not specific to this
/// function**: [`paint_panel`]'s own border is the same token against
/// the same canvas at the same ≈1.49:1, so it is the existing, accepted
/// tradeoff of a deliberately neutral, deliberately close-valued grey
/// theme (`design/themes/color-critical.toml`'s own header: "not
/// extreme contrast, just non-biasing chroma"). Raising it would mean
/// changing that theme's `border.default`, a design-owner decision
/// (PRD FR-027 *Ownership*), not this function's.
///
/// **Why the conditional [`control_outline`] is kept as well**, unlike
/// [`paint_panel`], which has only the one border: the two High
/// Contrast themes set `border.control_opacity = 1.0` with
/// `border.control` at pure white/black, which is their brief's
/// "mandatory strong borders on every control" taken literally. Dropping
/// it to match `paint_panel` exactly would have *downgraded* those two
/// themes from a 21:1 outline to `border.default`'s `hc.mid_gray`. So a
/// dialog paints two shapes in Dark/Light/Colour-Critical and three in
/// the two High Contrast themes, the third drawn last and therefore on
/// top — coincident with the second, and deliberately so.
///
/// **These are two functions rather than one token-parameterized helper
/// for a documentation reason, not a testing one.** An earlier version
/// of this comment said "do not simplify," implying a shared helper
/// would make some test vacuous; that was imprecise — nothing here
/// depends on the duplication, and no drift between the two has
/// occurred. The real reason is narrower: each function carries its own
/// `vocabulary.md` elevation citation next to the token it actually
/// resolves, which a shared helper would move away from both call
/// sites. Worth knowing either way: the two tokens resolve
/// *byte-identically* in three of the five built-in themes — Light
/// (both `neutral.900`), High Contrast Dark (both `hc.black`) and High
/// Contrast Light (both `hc.white`) — so in those three no
/// rendered-pixel test can tell this function's token choice from
/// [`paint_command_palette`]'s. Only Dark (`neutral.200` vs
/// `neutral.300`) and Colour-Critical (`cc.raised` `#4c4c4c` vs
/// `cc.overlay` `#5a5a5a`) distinguish them, which is why
/// `a_dialog_paints_surface_overlay_not_the_command_palettes_surface_
/// raised` is scoped to exactly those two and opens with an explicit
/// `assert_ne!` on the tokens so it cannot quietly become a tautology.
///
/// Still a real, honest gap, the same one [`paint_command_palette`]
/// has: neither the dialog's title nor its message is drawn (no text
/// shaping in this crate at all), and nothing here paints a scrim
/// behind the dialog — see `widgets::dialog`'s own module doc comment.
fn paint_dialog(
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> Result<Vec<Paint>, WidgetError> {
    // The same 1.0 logical px `paint_panel` strokes its own border at,
    // and a plain engineering default for the same reason: no "border
    // width" token exists in `design/tokens/scales.toml` yet.
    const BORDER_WIDTH: f32 = 1.0;

    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.md as f32,
    );
    let tolerance = tolerance_for_scale_factor(scale_factor);
    let fill_mesh = fill(&path, tolerance).map_err(WidgetError::Paint)?;
    let [fr, fg, fb] = theme.surface.overlay.to_srgb_f32();

    let border_mesh = stroke(&path, BORDER_WIDTH, tolerance).map_err(WidgetError::Paint)?;
    let [br, bg, bb] = theme.border.default.to_srgb_f32();

    let mut paints = vec![
        (fill_mesh, [fr, fg, fb, 1.0]),
        (border_mesh, [br, bg, bb, 1.0]),
    ];
    if let Some(outline) = control_outline(&path, theme, 1.0, scale_factor)? {
        paints.push(outline);
    }
    Ok(paints)
}

/// A colour swatch's own fill: `state.color` itself — the one widget in
/// this module whose fill colour is *not* a `Theme` token (see this
/// module's own doc comment and `widgets::color_swatch`'s for why: the
/// displayed colour is arbitrary caller data, e.g. the document's
/// current foreground colour, not UI chrome). `scales.radius.sm`, the
/// same small-control radius `Button`/`Checkbox`/`TextField` already
/// use — a swatch is a small control, not a floating panel like
/// `CommandPalette`. `theme` is still needed for `state.disabled_opacity`,
/// which *is* real chrome (how strongly a disabled swatch dims), not
/// the colour it swatches.
fn paint_color_swatch(
    state: ColorSwatchState,
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> Result<Vec<Paint>, WidgetError> {
    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.sm as f32,
    );
    let tolerance = tolerance_for_scale_factor(scale_factor);
    let mesh = fill(&path, tolerance).map_err(WidgetError::Paint)?;
    let [r, g, b] = state.color.to_srgb_f32();
    let alpha = if state.disabled {
        theme.state.disabled_opacity
    } else {
        1.0
    };
    let mut paints = vec![(mesh, [r, g, b, alpha])];
    if let Some(outline) = control_outline(&path, theme, alpha, scale_factor)? {
        paints.push(outline);
    }
    Ok(paints)
}

/// A list row's own highlight — nothing at all when `state.selected` is
/// `false` (a real, deliberate "no paint," the same convention
/// `WidgetKind::Container` uses, not a transparent fill), since an
/// unselected row is indistinguishable from its own owning widget's
/// background and needs no shape of its own. A selected row paints
/// `accent.primary` — `design/tokens/vocabulary.md`'s own entry for
/// that token names "selection highlight" explicitly, alongside
/// "primary buttons, active tool," so this isn't a new use invented
/// here. `scales.radius.sm`, the same small-control radius every other
/// non-panel shape in this module uses.
///
/// **The fill really is the row's whole box, and deliberately so** —
/// unlike [`paint_tree_item`], which clamps to one row's height because
/// a tree row's box grows to contain its children. A list row has no
/// children to contain, and a command-palette row's box is *meant* to be
/// its whole share of a sparse palette (`widgets::command_palette`'s own
/// `row_style` sets `flex_grow: 1.0`), so clamping here would leave a
/// palette row highlighted over only part of its own click target.
/// Measured rather than argued: applying `row_height(scales).min(bounds.
/// height)` here fails ten existing `tests/gallery.rs` cases, five of
/// them golden-image comparisons.
///
/// Staying inside the *panel* is a different question, and it is
/// [`clip_to_clipping_ancestors`]' job for every widget kind at once —
/// including `TreeItem`, whose own height clamp never addressed it.
fn paint_list_row(
    state: ListRowState,
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> Result<Vec<Paint>, WidgetError> {
    if !state.selected {
        return Ok(vec![]);
    }
    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.sm as f32,
    );
    let tolerance = tolerance_for_scale_factor(scale_factor);
    let mesh = fill(&path, tolerance).map_err(WidgetError::Paint)?;
    let [r, g, b] = theme.accent.primary.to_srgb_f32();
    let alpha = if state.disabled {
        theme.state.disabled_opacity
    } else {
        1.0
    };
    Ok(vec![(mesh, [r, g, b, alpha])])
}

/// A tree row's own highlight — the same shape [`paint_list_row`]
/// paints, for the same reason and from the same token: nothing at all
/// when the row isn't selected (a real, deliberate `Ok(vec![])`),
/// `accent.primary` at `scales.radius.sm` when it is,
/// `state.disabled_opacity` folded into the alpha when it's disabled.
///
/// **The one real difference, and it is load-bearing: the fill is one
/// row tall, not the whole box.** A tree row's own layout box grows to
/// contain its children (`widgets::tree_view::style` — that is what
/// makes a subtree's rows nest and indent in the first place), so a
/// selected *group* has bounds spanning every descendant beneath it.
/// Painting `bounds.height` would lay an opaque `accent.primary`
/// rectangle over that whole subtree — every descendant's own highlight
/// included, since `WidgetTree::paint_order` draws a parent before its
/// children only for the fill order, and this fill is opaque. Clamping
/// to `row_height(scales)` paints exactly the row itself. The
/// `.min(bounds.height)` guard keeps a row that is somehow *shorter*
/// than one line (a caller-supplied `set_bounds`, a squeezed layout)
/// from painting outside its own bounds.
///
/// **What it does not do is keep the row inside its own *panel*.** For a
/// 21 px row in a 13 px panel body it computes `min(21, 21) = 21` and
/// overhangs by 8 px, exactly as an unclamped `ListRow` would; that
/// class of overflow belongs to [`clip_to_clipping_ancestors`], which
/// runs before this function and hands it already-clipped `bounds`.
///
/// **A latent interaction, not yet reachable**: `clip_to_clipping_ancestors`
/// can move `bounds.y` down when a row is clipped at its *top* (a
/// scrolled-past-the-start row, once a scrolling container exists — see
/// `history_panel.rs`'s own disclosed damage-rect gap for the sibling case
/// clipping already guards). This function always paints `row_height`
/// starting from whatever `bounds.y` it receives, so a group row clipped
/// at the top would have its one-row highlight anchored over its first
/// visible *descendants* rather than its own (now-scrolled-off) row. No
/// caller can produce a top-clipped row today — nothing in this crate
/// scrolls, and every panel body only ever clips at the *bottom* — so this
/// is recorded rather than fixed.
fn paint_tree_item(
    state: &TreeItemState,
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> Result<Vec<Paint>, WidgetError> {
    if !state.selected {
        return Ok(vec![]);
    }
    let height = row_height(scales).min(bounds.height as f32);
    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        height,
        scales.radius.sm as f32,
    );
    let tolerance = tolerance_for_scale_factor(scale_factor);
    let mesh = fill(&path, tolerance).map_err(WidgetError::Paint)?;
    let [r, g, b] = theme.accent.primary.to_srgb_f32();
    let alpha = if state.disabled {
        theme.state.disabled_opacity
    } else {
        1.0
    };
    Ok(vec![(mesh, [r, g, b, alpha])])
}

/// A docked panel's own flat background, plus a real outline —
/// `surface.panel`/`border.default`, `design/tokens/vocabulary.md`'s
/// own entries for exactly this ("Default panel background (Layers,
/// Properties, Tool Options, ...)"; "Emphasized borders" is
/// `border.strong`'s own job, not this one's), not uses invented here.
/// `scales.radius.sm`, the same small-control radius every other
/// non-floating shape in this module already uses (a docked panel
/// reads as a small, fixed region, not a floating popover like
/// `CommandPalette`'s own `scales.radius.md`).
///
/// **The border is a real, necessary follow-on, not decoration**: a
/// plain fill alone was found — on real hardware, not assumed — to be
/// nearly invisible against the window's own background
/// (`surface.app`, `#1a1a1b`, next to `surface.panel`'s own original
/// `#212124` — a ~7-in-255 per-channel difference), the deliberately
/// "quiet" neutral ramp `design/tokens/palette.toml`'s own comment
/// names ("must stay quiet enough that they never compete with a
/// user's image"). Cahya's own call (`AskUserQuestion`), once the real
/// numbers were in front of him, was to add real definition via
/// `border.default` rather than either leave it or brighten the fill
/// itself (a design decision either way, not this crate's to make
/// alone). Stroke width (`1.0` logical px) is a plain engineering
/// default — no "border width" token exists in `design/tokens/
/// scales.toml` yet, and a one-pixel hairline is standard UI practice,
/// not really a design decision to raise the way an arbitrary size or
/// colour would be.
///
/// **The fill itself was later found — again on real hardware — to
/// still read as almost identical to the undocked rail beside it**,
/// even with the border in place (only the outline was visibly
/// toggling, not a perceived box). Cahya's own second call was to
/// shift `surface.panel`/`surface.raised`/`surface.overlay` up one
/// ramp step each in `design/themes/dark.toml` (`neutral.100` ->
/// `neutral.150`, `.150` -> `.200`, `.200` -> `.300`) rather than
/// reuse `surface.raised`'s old value outright, which would have
/// collapsed panels and popovers to the same colour. `surface.panel`
/// is `#28282c` now, not `#212124`.
///
/// No `disabled` state — a panel has no such concept — and no distinct
/// paint for collapsed vs. expanded: a real collapsed panel's own root
/// already resolves to a near-zero-height rect (`aurora_ui::
/// set_panel_collapsed`'s own `flex_grow: 0.0`), so an unconditional
/// fill-plus-border here already reads as "nothing visible" without
/// this function needing to know about collapse at all.
fn paint_panel(
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
    scale_factor: f32,
) -> Result<Vec<Paint>, WidgetError> {
    const BORDER_WIDTH: f32 = 1.0;

    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.sm as f32,
    );
    let tolerance = tolerance_for_scale_factor(scale_factor);
    let fill_mesh = fill(&path, tolerance).map_err(WidgetError::Paint)?;
    let [fr, fg, fb] = theme.surface.panel.to_srgb_f32();

    let border_mesh = stroke(&path, BORDER_WIDTH, tolerance).map_err(WidgetError::Paint)?;
    let [br, bg, bb] = theme.border.default.to_srgb_f32();

    Ok(vec![
        (fill_mesh, [fr, fg, fb, 1.0]),
        (border_mesh, [br, bg, bb, 1.0]),
    ])
}

#[cfg(test)]
mod tests {
    use super::{Paint, paint_widget};
    use crate::tree::{WidgetId, WidgetTree};
    use crate::widgets::{
        CommandEntry, DialogAction, DialogHandle, ListRowState, ScrollbarRange, ScrollbarState,
        WidgetKind, command_palette_state, insert_button, insert_checkbox, insert_color_swatch,
        insert_command_palette, insert_container, insert_dialog, insert_scrollbar, insert_slider,
        insert_text_field, insert_tree_item, insert_tree_view, new_tree, row_height,
        set_button_disabled, set_button_pressed, set_checkbox_disabled, set_color_swatch_disabled,
        set_scrollbar_disabled, set_scrollbar_value, set_slider_disabled, set_slider_value,
        set_text_field_disabled, set_tree_item_disabled, set_tree_item_selected, toggle_checkbox,
    };
    use accesskit::{Orientation, Toggled};
    use aurora_core::Rect;
    use aurora_theme::{Color, Palette, Scales, Theme, ThemeSet};

    const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
    const DARK_THEME_TOML: &str = include_str!("../../../design/themes/dark.toml");
    const COLOR_CRITICAL_THEME_TOML: &str =
        include_str!("../../../design/themes/color-critical.toml");
    const LIGHT_THEME_TOML: &str = include_str!("../../../design/themes/light.toml");
    const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");

    fn dark_theme() -> Theme {
        let palette = match Palette::from_toml_str(PALETTE_TOML) {
            Ok(palette) => palette,
            Err(err) => unreachable!("the committed palette must parse: {err:?}"),
        };
        let mut themes = ThemeSet::new();
        if let Err(err) = themes.register(DARK_THEME_TOML) {
            unreachable!("the committed Dark theme must register: {err:?}");
        }
        match themes.resolve("Dark", &palette) {
            Ok(theme) => theme,
            Err(err) => unreachable!("the committed Dark theme must resolve: {err:?}"),
        }
    }

    fn scales() -> Scales {
        match Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("the committed scales must parse: {err:?}"),
        }
    }

    /// A synthetic child of the real, committed Dark theme with
    /// `border.control_opacity` raised above `0.0` -- proves
    /// `control_outline` actually turns the outline on, without
    /// inventing a real second High Contrast design (Cahya's own call
    /// per FR-027 *Ownership*, not this test's to make). The same
    /// "synthetic child theme" pattern `aurora_theme::theme`'s own tests
    /// already use to prove `extends` merging generically.
    fn high_contrast_theme() -> Theme {
        let palette = match Palette::from_toml_str(PALETTE_TOML) {
            Ok(palette) => palette,
            Err(err) => unreachable!("the committed palette must parse: {err:?}"),
        };
        let mut themes = ThemeSet::new();
        if let Err(err) = themes.register(DARK_THEME_TOML) {
            unreachable!("the committed Dark theme must register: {err:?}");
        }
        let child = r#"
            schema_version = 1
            name = "TestHighContrast"
            extends = "Dark"
            is_default = false

            [border]
            control = "neutral.900"
            control_opacity = 1.0
        "#;
        if let Err(err) = themes.register(child) {
            unreachable!("{err:?}");
        }
        match themes.resolve("TestHighContrast", &palette) {
            Ok(theme) => theme,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// The real, committed Colour-Critical theme -- the *second* of the
    /// only two built-in themes that resolve `surface.overlay` and
    /// `surface.raised` to different values (Dark is the first), which
    /// is the only reason this module needs a third real theme fixture
    /// at all. `extends = "Dark"`, so the parent has to be registered
    /// first, and the name is the exact, case-sensitive
    /// `"Color-Critical"` that file's own `name` field holds.
    fn color_critical_theme() -> Theme {
        let palette = match Palette::from_toml_str(PALETTE_TOML) {
            Ok(palette) => palette,
            Err(err) => unreachable!("the committed palette must parse: {err:?}"),
        };
        let mut themes = ThemeSet::new();
        if let Err(err) = themes.register(DARK_THEME_TOML) {
            unreachable!("the committed Dark theme must register: {err:?}");
        }
        if let Err(err) = themes.register(COLOR_CRITICAL_THEME_TOML) {
            unreachable!("the committed Color-Critical theme must register: {err:?}");
        }
        match themes.resolve("Color-Critical", &palette) {
            Ok(theme) => theme,
            Err(err) => unreachable!("the committed Color-Critical theme must resolve: {err:?}"),
        }
    }

    /// The real, committed Light theme. One test needs it --
    /// `a_light_theme_dialog_is_not_invisible_against_the_panel_behind_it`
    /// -- because Light is the theme where every elevated surface token
    /// collapses onto the same `neutral.900` value *and*
    /// `border.control_opacity` is `0.0`. `extends = "Dark"`, so the
    /// parent has to be registered first, same as `color_critical_theme`
    /// above.
    fn light_theme() -> Theme {
        let palette = match Palette::from_toml_str(PALETTE_TOML) {
            Ok(palette) => palette,
            Err(err) => unreachable!("the committed palette must parse: {err:?}"),
        };
        let mut themes = ThemeSet::new();
        if let Err(err) = themes.register(DARK_THEME_TOML) {
            unreachable!("the committed Dark theme must register: {err:?}");
        }
        if let Err(err) = themes.register(LIGHT_THEME_TOML) {
            unreachable!("the committed Light theme must register: {err:?}");
        }
        match themes.resolve("Light", &palette) {
            Ok(theme) => theme,
            Err(err) => unreachable!("the committed Light theme must resolve: {err:?}"),
        }
    }

    /// Resolves `id`'s own paint and asserts it's exactly one shape --
    /// `Button`/`Checkbox`'s own case -- returning that shape.
    fn single_paint(
        tree: &WidgetTree<WidgetKind>,
        id: WidgetId,
        theme: &Theme,
        scales: &Scales,
        scale_factor: f32,
    ) -> Paint {
        let mut paints = match paint_widget(tree, id, theme, scales, scale_factor) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(paints.len(), 1, "expected exactly one shape: {paints:?}");
        paints.remove(0)
    }

    #[test]
    // Both sides go through the exact same `Color::to_srgb_f32` call on
    // the same underlying u8 channels -- bit-exact, not accumulated
    // float noise, the same precedent `aurora_color`'s own round-trip
    // tests already allow this lint for.
    #[allow(clippy::float_cmp)]
    fn a_laid_out_button_paints_a_non_empty_mesh_in_its_own_accent_colour() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let button = match insert_button(&mut tree, root, &scales, "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            button,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 32,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (mesh, color) = single_paint(&tree, button, &theme, &scales, 1.0);
        assert!(
            !mesh.vertices.is_empty() && !mesh.indices.is_empty(),
            "an 80x32 button must tessellate to real geometry"
        );
        let [r, g, b] = theme.accent.primary.to_srgb_f32();
        assert_eq!(
            color,
            [r, g, b, 1.0],
            "an enabled, unpressed button must use accent.primary at full opacity"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_pressed_button_uses_the_active_accent_colour() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let button = match insert_button(&mut tree, root, &scales, "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_button_pressed(&mut tree, button, true) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (_, color) = single_paint(&tree, button, &theme, &scales, 1.0);
        let [r, g, b] = theme.accent.primary_active.to_srgb_f32();
        assert_eq!(color, [r, g, b, 1.0]);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_disabled_button_applies_the_theme_disabled_opacity() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let button = match insert_button(&mut tree, root, &scales, "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_button_disabled(&mut tree, button, true) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (_, color) = single_paint(&tree, button, &theme, &scales, 1.0);
        assert_eq!(color[3], theme.state.disabled_opacity);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_button_gains_a_second_outline_shape_when_border_control_opacity_is_above_zero() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let button = match insert_button(&mut tree, root, &scales, "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            button,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 32,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = high_contrast_theme();

        let paints = match paint_widget(&tree, button, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            paints.len(),
            2,
            "border.control_opacity above 0 must add a second, outline shape on top of the fill"
        );
        let Some((outline_mesh, outline_color)) = paints.get(1) else {
            unreachable!("just asserted len == 2");
        };
        assert!(
            !outline_mesh.vertices.is_empty() && !outline_mesh.indices.is_empty(),
            "the outline must tessellate to real geometry"
        );
        let [r, g, b] = theme.border.control.to_srgb_f32();
        assert_eq!(
            *outline_color,
            [r, g, b, theme.border.control_opacity],
            "the outline must use border.control at border.control_opacity"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_disabled_buttons_outline_dims_by_the_same_disabled_opacity_as_its_fill() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let button = match insert_button(&mut tree, root, &scales, "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_button_disabled(&mut tree, button, true) {
            unreachable!("{err:?}");
        }
        let theme = high_contrast_theme();

        let paints = match paint_widget(&tree, button, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(paints.len(), 2);
        let Some((_, outline_color)) = paints.get(1) else {
            unreachable!("just asserted len == 2");
        };
        assert_eq!(
            outline_color[3],
            theme.border.control_opacity * theme.state.disabled_opacity,
            "a disabled control's outline dims the same way its fill already does"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn dark_theme_has_border_control_opacity_at_zero() {
        // The load-bearing property this whole task rests on: the real,
        // committed Dark theme must resolve `border.control_opacity` to
        // exactly `0.0`, which is what makes every `control_outline`
        // call above return `None` and every pre-existing paint test's
        // shape count stay unchanged.
        let theme = dark_theme();
        assert_eq!(theme.border.control_opacity, 0.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn an_unchecked_checkbox_paints_surface_sunken() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let checkbox = match insert_checkbox(&mut tree, root, &scales, "x") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            checkbox,
            Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 20,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (mesh, color) = single_paint(&tree, checkbox, &theme, &scales, 1.0);
        assert!(
            !mesh.vertices.is_empty() && !mesh.indices.is_empty(),
            "a 20x20 checkbox must tessellate to real geometry"
        );
        let [r, g, b] = theme.surface.sunken.to_srgb_f32();
        assert_eq!(
            color,
            [r, g, b, 1.0],
            "an unchecked, enabled checkbox must use surface.sunken at full opacity"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_checked_checkbox_paints_accent_primary() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let checkbox = match insert_checkbox(&mut tree, root, &scales, "x") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = toggle_checkbox(&mut tree, checkbox) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (_, color) = single_paint(&tree, checkbox, &theme, &scales, 1.0);
        let [r, g, b] = theme.accent.primary.to_srgb_f32();
        assert_eq!(color, [r, g, b, 1.0]);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_mixed_checkbox_paints_the_same_as_checked() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let checkbox = match insert_checkbox(&mut tree, root, &scales, "x") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(WidgetKind::Checkbox(state)) = tree.payload_mut(checkbox) else {
            unreachable!("just inserted");
        };
        state.checked = Toggled::Mixed;
        let theme = dark_theme();

        let (_, color) = single_paint(&tree, checkbox, &theme, &scales, 1.0);
        let [r, g, b] = theme.accent.primary.to_srgb_f32();
        assert_eq!(
            color,
            [r, g, b, 1.0],
            "Mixed currently renders identically to True -- see this module's own doc comment"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_disabled_checkbox_applies_the_theme_disabled_opacity() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let checkbox = match insert_checkbox(&mut tree, root, &scales, "x") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_checkbox_disabled(&mut tree, checkbox, true) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (_, color) = single_paint(&tree, checkbox, &theme, &scales, 1.0);
        assert_eq!(color[3], theme.state.disabled_opacity);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_laid_out_slider_paints_a_track_then_a_thumb() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let slider = match insert_slider(&mut tree, root, &scales, "vol", 50.0, 0.0, 100.0) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            slider,
            Rect {
                x: 0,
                y: 0,
                width: 200,
                height: 20,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let mut paints = match paint_widget(&tree, slider, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(paints.len(), 2, "a slider paints a track and a thumb");
        let (thumb_mesh, thumb_color) = paints.remove(1);
        let (track_mesh, track_color) = paints.remove(0);
        assert!(
            !track_mesh.vertices.is_empty() && !track_mesh.indices.is_empty(),
            "the track must tessellate to real geometry"
        );
        assert!(
            !thumb_mesh.vertices.is_empty() && !thumb_mesh.indices.is_empty(),
            "the thumb must tessellate to real geometry"
        );
        let [r, g, b] = theme.surface.sunken.to_srgb_f32();
        assert_eq!(
            track_color,
            [r, g, b, 1.0],
            "the track must use surface.sunken, the same recessed-control token an unchecked \
             checkbox already uses"
        );
        let [r, g, b] = theme.accent.primary.to_srgb_f32();
        assert_eq!(
            thumb_color,
            [r, g, b, 1.0],
            "the thumb must use accent.primary at full opacity"
        );
    }

    #[test]
    fn a_sliders_thumb_moves_right_as_its_value_increases() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let slider = match insert_slider(&mut tree, root, &scales, "vol", 0.0, 0.0, 100.0) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            slider,
            Rect {
                x: 0,
                y: 0,
                width: 200,
                height: 20,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let thumb_min_x = |tree: &WidgetTree<WidgetKind>| -> f32 {
            let mut paints = match paint_widget(tree, slider, &theme, &scales, 1.0) {
                Ok(paints) => paints,
                Err(err) => unreachable!("{err:?}"),
            };
            assert_eq!(paints.len(), 2);
            let (thumb_mesh, _) = paints.remove(1);
            thumb_mesh
                .vertices
                .iter()
                .map(|point| point.x)
                .fold(f32::INFINITY, f32::min)
        };

        let at_min = thumb_min_x(&tree);
        if let Err(err) = set_slider_value(&mut tree, slider, 100.0) {
            unreachable!("{err:?}");
        }
        let at_max = thumb_min_x(&tree);
        assert!(
            at_max > at_min,
            "the thumb must move right as the value increases: {at_min} -> {at_max}"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_disabled_slider_applies_the_theme_disabled_opacity_to_both_shapes() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let slider = match insert_slider(&mut tree, root, &scales, "vol", 0.0, 0.0, 100.0) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_slider_disabled(&mut tree, slider, true) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let paints = match paint_widget(&tree, slider, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(paints.len(), 2);
        for (_, color) in &paints {
            assert_eq!(color[3], theme.state.disabled_opacity);
        }
    }

    /// A vertical scrollbar over a 0..=100 range showing a 20-unit page
    /// — the shared fixture the three scrollbar paint tests below use.
    fn scrollbar_range() -> ScrollbarRange {
        ScrollbarRange {
            min: 0.0,
            max: 100.0,
            page_size: 20.0,
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_laid_out_vertical_scrollbar_paints_a_track_then_a_thumb() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let scrollbar = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            None,
            0.0,
            scrollbar_range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            scrollbar,
            Rect {
                x: 0,
                y: 0,
                width: 13,
                height: 300,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let mut paints = match paint_widget(&tree, scrollbar, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(paints.len(), 2, "a scrollbar paints a track and a thumb");
        let (thumb_mesh, thumb_color) = paints.remove(1);
        let (track_mesh, track_color) = paints.remove(0);
        assert!(
            !track_mesh.vertices.is_empty() && !track_mesh.indices.is_empty(),
            "the track must tessellate to real geometry"
        );
        assert!(
            !thumb_mesh.vertices.is_empty() && !thumb_mesh.indices.is_empty(),
            "the thumb must tessellate to real geometry"
        );
        let [r, g, b] = theme.surface.sunken.to_srgb_f32();
        assert_eq!(
            track_color,
            [r, g, b, 1.0],
            "the track must use surface.sunken, the same recessed-control token a slider's own \
             track already uses"
        );
        let [r, g, b] = theme.accent.primary.to_srgb_f32();
        assert_eq!(
            thumb_color,
            [r, g, b, 1.0],
            "the thumb must use accent.primary at full opacity"
        );

        // A 20-of-120 page over a 300px track is a 50px thumb -- shorter
        // than the track, which is the whole visual point of a
        // scrollbar as against a slider.
        let thumb_top = thumb_mesh
            .vertices
            .iter()
            .map(|point| point.y)
            .fold(f32::INFINITY, f32::min);
        let thumb_bottom = thumb_mesh
            .vertices
            .iter()
            .map(|point| point.y)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            thumb_bottom - thumb_top < 300.0,
            "a partial page must give a thumb shorter than its own track: \
             {thumb_top} -> {thumb_bottom}"
        );
    }

    #[test]
    fn a_scrollbars_thumb_moves_along_its_track_as_its_value_increases() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let scrollbar = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            None,
            0.0,
            scrollbar_range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            scrollbar,
            Rect {
                x: 0,
                y: 0,
                width: 13,
                height: 300,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let thumb_min_y = |tree: &WidgetTree<WidgetKind>| -> f32 {
            let mut paints = match paint_widget(tree, scrollbar, &theme, &scales, 1.0) {
                Ok(paints) => paints,
                Err(err) => unreachable!("{err:?}"),
            };
            assert_eq!(paints.len(), 2);
            let (thumb_mesh, _) = paints.remove(1);
            thumb_mesh
                .vertices
                .iter()
                .map(|point| point.y)
                .fold(f32::INFINITY, f32::min)
        };

        let at_min = thumb_min_y(&tree);
        if let Err(err) = set_scrollbar_value(&mut tree, scrollbar, 100.0) {
            unreachable!("{err:?}");
        }
        let at_max = thumb_min_y(&tree);
        assert!(
            at_max > at_min,
            "a vertical scrollbar's thumb must move down as the value increases: \
             {at_min} -> {at_max}"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_disabled_scrollbar_applies_the_theme_disabled_opacity_to_both_shapes() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let scrollbar = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Horizontal,
            None,
            0.0,
            scrollbar_range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_scrollbar_disabled(&mut tree, scrollbar, true) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let paints = match paint_widget(&tree, scrollbar, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(paints.len(), 2);
        for (_, color) in &paints {
            assert_eq!(color[3], theme.state.disabled_opacity);
        }
    }

    /// A mesh's own axis-aligned bounding box, `(min_x, min_y, max_x,
    /// max_y)` — every scrollbar geometry assertion below is about
    /// where a thumb actually landed, not just that something
    /// tessellated.
    type Bbox = (f32, f32, f32, f32);

    /// One case for `a_scrollbar_with_non_finite_state_still_paints_
    /// finite_geometry`: a name for the failure message, and the edit
    /// that puts a `ScrollbarState` into that shape.
    type ScrollbarCase = (&'static str, Box<dyn FnOnce(&mut ScrollbarState)>);

    fn bbox(mesh: &aurora_vector::Mesh) -> Bbox {
        let mut bounds = (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        );
        for point in &mesh.vertices {
            bounds.0 = bounds.0.min(point.x);
            bounds.1 = bounds.1.min(point.y);
            bounds.2 = bounds.2.max(point.x);
            bounds.3 = bounds.3.max(point.y);
        }
        bounds
    }

    /// Builds a laid-out scrollbar of exactly `bounds` and paints it,
    /// returning `(track_bbox, thumb_bbox)`. `state` is applied through
    /// `payload_mut` *after* insertion deliberately: several cases below
    /// are ranges `insert_scrollbar` now refuses outright, and the point
    /// is that `paint_scrollbar` survives them anyway — `payload_mut` is
    /// public, so "unreachable through the constructor" is not the same
    /// as "unreachable."
    fn scrollbar_geometry(
        bounds: Rect,
        orientation: Orientation,
        edit: impl FnOnce(&mut ScrollbarState),
    ) -> (Bbox, Bbox) {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            orientation,
            None,
            0.0,
            scrollbar_range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.payload_mut(id) {
            Some(WidgetKind::Scrollbar(state)) => edit(state),
            other => unreachable!("expected Scrollbar, got {other:?}"),
        }
        if let Err(err) = tree.set_bounds(id, bounds) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();
        let mut paints = match paint_widget(&tree, id, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(paints.len(), 2, "a scrollbar paints a track and a thumb");
        let (thumb_mesh, _) = paints.remove(1);
        let (track_mesh, _) = paints.remove(0);
        (bbox(&track_mesh), bbox(&thumb_mesh))
    }

    /// A 13x300 vertical bar — the shape the geometry tests below use,
    /// matching `insert_scrollbar`'s own 13px type-scale thickness.
    const TALL_BAR: Rect = Rect {
        x: 0,
        y: 0,
        width: 13,
        height: 300,
    };

    /// `TALL_BAR` transposed — every geometry test above exercises only
    /// `Orientation::Vertical`; this is the one case pinning that the
    /// horizontal branch (thumb offset along `x`, thickness on `y`)
    /// resolves correctly too, independent of the GPU-gated gallery
    /// (which self-skips on a CPU-only runner).
    const WIDE_BAR: Rect = Rect {
        x: 0,
        y: 0,
        width: 300,
        height: 13,
    };

    /// The horizontal counterpart of the geometry this module's doc
    /// comment traces by hand for the vertical case: a 20-of-120 page
    /// (`(max - min) + page_size`) over a 300px track is a 50px thumb,
    /// and a value of 50 out of a 0..100 range centers it at the
    /// `x = [125, 175]` offset the same arithmetic gives vertically.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_horizontal_scrollbars_thumb_sits_on_the_x_axis_not_the_y_axis() {
        let (track, thumb) = scrollbar_geometry(WIDE_BAR, Orientation::Horizontal, |state| {
            state.min = 0.0;
            state.max = 100.0;
            state.value = 50.0;
            state.page_size = 20.0;
        });
        assert_eq!(track, (0.0, 0.0, 300.0, 13.0));
        assert_eq!(
            thumb,
            (125.0, 0.0, 175.0, 13.0),
            "the thumb travels along x and keeps the bar's own y extent, \
             not the vertical branch's x/y swapped"
        );
    }

    /// `min == max` with no page at all: nothing is scrollable, so
    /// there is no proportional information to draw a short thumb from
    /// and the thumb covers the whole track. Kills the `span > 0.0`
    /// guard's removal (`0.0 / 0.0` is `NaN`, which `f32::max` silently
    /// launders into the 13px thickness floor — a 13px thumb, not a
    /// 300px one) and the `range > 0.0` guard's removal (`NaN` offset,
    /// which reaches `lyon` and fails the paint outright).
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_scrollbar_with_nothing_to_scroll_paints_a_full_length_thumb() {
        let (track, thumb) = scrollbar_geometry(TALL_BAR, Orientation::Vertical, |state| {
            state.min = 50.0;
            state.max = 50.0;
            state.value = 50.0;
            state.page_size = 0.0;
        });
        assert_eq!(track, (0.0, 0.0, 13.0, 300.0));
        assert_eq!(
            thumb,
            (0.0, 0.0, 13.0, 300.0),
            "with nothing to scroll the thumb fills its own track"
        );
    }

    /// A zero page is "no proportional information" (`ScrollbarRange::
    /// page_size`'s own doc comment), which must still leave something
    /// grabbable rather than a zero-length thumb. Kills the
    /// `.max(thickness)` floor's removal.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_scrollbar_with_a_zero_page_size_still_paints_a_grabbable_thumb() {
        let (_, thumb) = scrollbar_geometry(TALL_BAR, Orientation::Vertical, |state| {
            state.page_size = 0.0;
            state.value = 0.0;
        });
        assert_eq!(
            thumb,
            (0.0, 0.0, 13.0, 13.0),
            "the thumb is floored at the bar's own cross-axis thickness, not zero"
        );
    }

    /// A page larger than the travel it sits in is a caller saying the
    /// viewport shows more than the content — the thumb grows towards
    /// the whole track but must never exceed it.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_page_larger_than_the_travel_paints_a_longer_thumb_but_never_a_longer_track() {
        let (_, small_page) = scrollbar_geometry(TALL_BAR, Orientation::Vertical, |state| {
            state.min = 0.0;
            state.max = 100.0;
            state.value = 0.0;
            state.page_size = 20.0;
        });
        let (track, big_page) = scrollbar_geometry(TALL_BAR, Orientation::Vertical, |state| {
            state.min = 0.0;
            state.max = 100.0;
            state.value = 0.0;
            state.page_size = 1000.0;
        });
        assert!(
            big_page.3 - big_page.1 > small_page.3 - small_page.1,
            "a bigger page must give a longer thumb: {small_page:?} -> {big_page:?}"
        );
        assert!(
            big_page.3 <= track.3 && big_page.1 >= track.1,
            "the thumb must stay inside its own track: {big_page:?} in {track:?}"
        );
    }

    /// A bar thicker than it is long — a real possibility for a
    /// horizontal bar in a narrow column, and the case where the
    /// thumb's own `.max(thickness)` floor fights its own track. Kills
    /// the `.min(track_len)` cap's removal, which would paint a 40px
    /// thumb overhanging a 20px track by its own length again.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_bar_thicker_than_it_is_long_keeps_its_thumb_inside_its_track() {
        let squat = Rect {
            x: 5,
            y: 7,
            width: 40,
            height: 20,
        };
        let (track, thumb) = scrollbar_geometry(squat, Orientation::Vertical, |state| {
            state.value = 100.0;
        });
        assert_eq!(track, (5.0, 7.0, 45.0, 27.0));
        assert_eq!(
            thumb, track,
            "a thumb floored at a thickness bigger than its own track is capped at the track"
        );
    }

    /// An inverted range (`min > max`, reachable only through
    /// `payload_mut` now that `insert_scrollbar` refuses it) makes the
    /// scrollable span itself *negative*, which is the one case the
    /// `span > 0.0` guard decides differently from the non-finite
    /// fallback that follows it: without the guard, `page_size /
    /// negative_span` is a perfectly finite negative number that clamps
    /// to `0.0` and paints a 13px stub. The documented convention for
    /// every degenerate input is one full-length thumb parked at the
    /// start, so that is what is pinned here.
    #[test]
    #[allow(clippy::float_cmp)]
    fn an_inverted_range_paints_a_full_length_thumb_parked_at_the_start() {
        let (track, thumb) = scrollbar_geometry(TALL_BAR, Orientation::Vertical, |state| {
            state.min = 100.0;
            state.max = 0.0;
            state.value = 50.0;
        });
        assert_eq!(track, (0.0, 0.0, 13.0, 300.0));
        assert_eq!(thumb, track);
    }

    /// The `range > 0.0` guard is deliberately kept but is, as of this
    /// round, *provably unobservable*, and saying so is more useful than
    /// pretending a test covers it. Whenever `range == 0.0`, `span`
    /// reduces to `page_size`, so `thumb_fraction` is `page_size /
    /// page_size == 1.0` (or the zero-span fallback, also `1.0`) — a
    /// full-length thumb, whose remaining travel `(track_len -
    /// thumb_len).max(0.0)` is exactly zero. `offset` is then
    /// `anything * 0.0`, so no `position_fraction` the guard could
    /// return is distinguishable from any other. Deleting the guard is
    /// therefore an equivalent mutant, not an uncovered branch: it
    /// survives this suite because it *cannot* change the output, and it
    /// stays in the source because `range` becoming observable again
    /// (any future change to how `thumb_len` is derived) would make it
    /// load-bearing without warning. This test pins the observable half.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_zero_range_leaves_the_thumb_no_travel_whatever_its_value() {
        let (track, at_start) = scrollbar_geometry(TALL_BAR, Orientation::Vertical, |state| {
            state.min = 50.0;
            state.max = 50.0;
            state.value = 50.0;
        });
        let (_, past_the_end) = scrollbar_geometry(TALL_BAR, Orientation::Vertical, |state| {
            state.min = 50.0;
            state.max = 50.0;
            state.value = 1000.0;
        });
        assert_eq!(at_start, track);
        assert_eq!(past_the_end, track);
    }

    /// The build-profile-dependent panic this function's own doc
    /// comment describes: a `NaN` value, and infinite-but-ordered
    /// bounds, both produced `NaN` geometry that tripped `lyon`'s own
    /// `assert!(p.y.is_finite())` in a debug build (this one) and
    /// returned `Err(Paint)` in a release build. Neither is reachable
    /// through `insert_scrollbar` any more, but `payload_mut` is
    /// public, so `paint_scrollbar` is made total rather than merely
    /// unreached. Asserting on real finite geometry, not just `Ok`.
    #[test]
    fn a_scrollbar_with_non_finite_state_still_paints_finite_geometry() {
        let cases: Vec<ScrollbarCase> = vec![
            (
                "NaN value",
                Box::new(|state: &mut ScrollbarState| state.value = f64::NAN),
            ),
            (
                "infinite bounds",
                Box::new(|state: &mut ScrollbarState| {
                    state.min = f64::NEG_INFINITY;
                    state.max = f64::INFINITY;
                }),
            ),
            (
                "NaN page size",
                Box::new(|state: &mut ScrollbarState| state.page_size = f64::NAN),
            ),
            (
                "inverted bounds",
                Box::new(|state: &mut ScrollbarState| {
                    state.min = 100.0;
                    state.max = 0.0;
                }),
            ),
        ];
        for (name, edit) in cases {
            let (track, thumb) = scrollbar_geometry(TALL_BAR, Orientation::Vertical, edit);
            for value in [
                track.0, track.1, track.2, track.3, thumb.0, thumb.1, thumb.2, thumb.3,
            ] {
                assert!(value.is_finite(), "{name} produced non-finite geometry");
            }
            assert!(
                thumb.1 >= track.1 && thumb.3 <= track.3,
                "{name} put the thumb outside its own track: {thumb:?} in {track:?}"
            );
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_laid_out_text_field_paints_surface_sunken() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let field = match insert_text_field(&mut tree, root, &scales, "name", "hello") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            field,
            Rect {
                x: 0,
                y: 0,
                width: 160,
                height: 28,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (mesh, color) = single_paint(&tree, field, &theme, &scales, 1.0);
        assert!(
            !mesh.vertices.is_empty() && !mesh.indices.is_empty(),
            "a 160x28 text field must tessellate to real geometry"
        );
        let [r, g, b] = theme.surface.sunken.to_srgb_f32();
        assert_eq!(
            color,
            [r, g, b, 1.0],
            "an enabled text field must use surface.sunken at full opacity"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_disabled_text_field_applies_the_theme_disabled_opacity() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let field = match insert_text_field(&mut tree, root, &scales, "name", "") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_text_field_disabled(&mut tree, field, true) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (_, color) = single_paint(&tree, field, &theme, &scales, 1.0);
        assert_eq!(color[3], theme.state.disabled_opacity);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_laid_out_command_palette_paints_surface_raised() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let commands = vec![CommandEntry::new("edit.undo", "Undo")];
        let palette = match insert_command_palette(&mut tree, root, commands) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            palette,
            Rect {
                x: 0,
                y: 0,
                width: 320,
                height: 240,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (mesh, color) = single_paint(&tree, palette, &theme, &scales, 1.0);
        assert!(
            !mesh.vertices.is_empty() && !mesh.indices.is_empty(),
            "a 320x240 command palette must tessellate to real geometry"
        );
        let [r, g, b] = theme.surface.raised.to_srgb_f32();
        assert_eq!(
            color,
            [r, g, b, 1.0],
            "a command palette's own panel must use surface.raised at full opacity"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_command_palettes_selected_result_row_paints_accent_primary() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let commands = vec![CommandEntry::new("edit.undo", "Undo")];
        let palette = match insert_command_palette(&mut tree, root, commands) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let selected_row = match command_palette_state(&tree, palette) {
            Ok(state) => state.selected_row(),
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(row) = selected_row else {
            unreachable!("one command was inserted, so the first result is selected");
        };
        if let Err(err) = tree.set_bounds(
            row,
            Rect {
                x: 0,
                y: 0,
                width: 200,
                height: 24,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (mesh, color) = single_paint(&tree, row, &theme, &scales, 1.0);
        assert!(
            !mesh.vertices.is_empty() && !mesh.indices.is_empty(),
            "a 200x24 selected row must tessellate to real geometry"
        );
        let [r, g, b] = theme.accent.primary.to_srgb_f32();
        assert_eq!(
            color,
            [r, g, b, 1.0],
            "design/tokens/vocabulary.md names accent.primary for exactly this, \
             'selection highlight'"
        );
    }

    #[test]
    fn a_command_palettes_unselected_result_row_has_no_paint() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let commands = vec![
            CommandEntry::new("edit.undo", "Undo"),
            CommandEntry::new("edit.redo", "Redo"),
        ];
        let palette = match insert_command_palette(&mut tree, root, commands) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let body = match command_palette_state(&tree, palette) {
            Ok(state) => state.body(),
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(rows) = tree.children(body) else {
            unreachable!("just inserted");
        };
        let Some(&second_row) = rows.get(1) else {
            unreachable!("two commands were inserted, so a second row exists");
        };
        let theme = dark_theme();

        let paints = match paint_widget(&tree, second_row, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            paints.is_empty(),
            "only the selected row paints a highlight; an unselected row paints nothing"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_disabled_selected_list_row_applies_the_theme_disabled_opacity() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let row = match tree.insert(
            root,
            taffy::Style::default(),
            accesskit::Node::new(accesskit::Role::ListBoxOption),
            WidgetKind::ListRow(ListRowState {
                selected: true,
                disabled: true,
            }),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            row,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 20,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (_, color) = single_paint(&tree, row, &theme, &scales, 1.0);
        assert_eq!(
            color[3], theme.state.disabled_opacity,
            "a disabled, selected row still dims like every other disabled widget's paint"
        );
    }

    /// A selected tree row paints the same token a selected list row
    /// does — `accent.primary`, `design/tokens/vocabulary.md`'s own
    /// "selection highlight".
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_selected_tree_row_paints_accent_primary() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let row = match insert_tree_item(&mut tree, root, &scales, "Layer 1", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_selected(&mut tree, row, true) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.set_bounds(
            row,
            Rect {
                x: 0,
                y: 0,
                width: 200,
                height: 21,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (mesh, color) = single_paint(&tree, row, &theme, &scales, 1.0);
        assert!(
            !mesh.vertices.is_empty() && !mesh.indices.is_empty(),
            "a 200x21 selected tree row must tessellate to real geometry"
        );
        let [r, g, b] = theme.accent.primary.to_srgb_f32();
        assert_eq!(color, [r, g, b, 1.0]);
    }

    #[test]
    fn an_unselected_tree_row_has_no_paint() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let row = match insert_tree_item(&mut tree, root, &scales, "Layer 1", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let theme = dark_theme();
        let paints = match paint_widget(&tree, row, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            paints.is_empty(),
            "an unselected row paints nothing at all, the same as an unselected list row"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_disabled_selected_tree_row_applies_the_theme_disabled_opacity() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let row = match insert_tree_item(&mut tree, root, &scales, "Layer 1", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_selected(&mut tree, row, true) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_tree_item_disabled(&mut tree, row, true) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.set_bounds(
            row,
            Rect {
                x: 0,
                y: 0,
                width: 200,
                height: 21,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (_, color) = single_paint(&tree, row, &theme, &scales, 1.0);
        assert_eq!(color[3], theme.state.disabled_opacity);
    }

    /// The one real difference from `paint_list_row`, and the reason
    /// `paint_tree_item` exists at all: a selected *group*'s own layout
    /// box spans every descendant beneath it (that is what makes a
    /// subtree nest), so painting `bounds.height` would lay an opaque
    /// rectangle over all of them. Measured through a real
    /// `compute_layout`, not a hand-set `set_bounds`, so the group's
    /// bounds are the ones the layout engine actually produces.
    #[test]
    fn a_selected_groups_highlight_is_one_row_tall_not_its_whole_subtree() {
        let root_style = taffy::Style {
            size: taffy::Size {
                width: taffy::style_helpers::length(300.0_f32),
                height: taffy::style_helpers::length(200.0_f32),
            },
            ..Default::default()
        };
        let (mut tree, root) = new_tree(root_style);
        let scales = scales();
        // Through a real `Role::Tree` container, not straight off the
        // root: a `Row`-direction parent's own `align_items: Stretch`
        // would inflate a row's `auto` height to the whole 200px, which
        // is a property of the *parent*, not of `tree_view::style`.
        let view = match insert_tree_view(&mut tree, root, Some("Layers")) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let group = match insert_tree_item(&mut tree, view, &scales, "Group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        for label in ["Child A", "Child B"] {
            if let Err(err) = insert_tree_item(&mut tree, group, &scales, label, false) {
                unreachable!("{err:?}");
            }
        }
        if let Err(err) = set_tree_item_selected(&mut tree, group, true) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(300.0, 200.0);

        let Some(bounds) = tree.bounds(group) else {
            unreachable!("just laid out");
        };
        assert_eq!(
            bounds.height, 63,
            "the group's own box really does span its own row plus both children"
        );
        let theme = dark_theme();
        let (mesh, _) = single_paint(&tree, group, &theme, &scales, 1.0);
        let (_, top, _, bottom) = bbox(&mesh);
        assert!(
            (bottom - top - 21.0).abs() < 0.5,
            "the highlight must be one row tall (21px), not the whole 63px box: \
             {top} -> {bottom}"
        );
    }

    /// A panel body that hides its overflow, sized the way a real dock
    /// rail sizes one: a content-independent share, here deliberately
    /// shorter than the one-line `min_size.height` floor its rows carry.
    /// `overflow: Hidden` needs `Overflow::Scroll`'s sibling semantics
    /// only for clipping, not scrolling, which is all this exercises.
    fn clipping_body(height: f32) -> taffy::Style {
        taffy::Style {
            flex_direction: taffy::FlexDirection::Column,
            size: taffy::Size {
                width: taffy::style_helpers::length(200.0_f32),
                height: taffy::style_helpers::length(height),
            },
            overflow: taffy::Point {
                x: taffy::Overflow::Hidden,
                y: taffy::Overflow::Hidden,
            },
            ..Default::default()
        }
    }

    /// One row's own style, the same shape `aurora_ui::panel`'s shared
    /// `row_style` builds: an `auto` height with a hard one-line floor,
    /// which is exactly what lets a row out-grow an undersized body.
    /// (It was `aurora_ui::history_panel`'s until `0.77.4` moved it up to
    /// `panel` and gave it a second caller, the Properties panel.)
    ///
    /// **This is a deliberate replica, and it cannot be shared.**
    /// `aurora-widgets` sits *below* `aurora-ui` in the layering rule
    /// (`scripts/layering.json`, PRD §7.2), so importing the real
    /// function here is not merely awkward, it is forbidden — which also
    /// means nothing mechanical will notice if the two drift apart. The
    /// tests below are then testing a shape production may no longer
    /// have. Whoever changes `aurora_ui::panel::row_style` has to change
    /// this by hand; that manual step is the cost of the layering rule,
    /// not an oversight. What actually needs to match is the pair that
    /// makes the clip observable — an `auto` main size with a
    /// `length(row_height)` minimum under it — not the whole style.
    fn floored_row_style(scales: &Scales) -> taffy::Style {
        taffy::Style {
            size: taffy::Size {
                width: taffy::style_helpers::percent(1.0_f32),
                height: taffy::style_helpers::auto(),
            },
            min_size: taffy::Size {
                width: taffy::style_helpers::length(row_height(scales)),
                height: taffy::style_helpers::length(row_height(scales)),
            },
            ..Default::default()
        }
    }

    /// A selected `ListRow` whose own one-line floor makes it taller
    /// than the panel body holding it must still paint inside that body.
    /// Measured, not hypothetical: a real `aurora_ui::build_workspace`
    /// at an 800×40 window gives the History body 13 px and its rows
    /// 21 px each. Before `0.77.3` the fill was built straight from the
    /// row's own unclipped bounds and hung 8 px over whatever was docked
    /// below.
    #[test]
    fn a_selected_list_rows_highlight_stays_inside_a_body_that_clips_its_overflow() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let Ok(body) = insert_container(&mut tree, root, clipping_body(13.0)) else {
            unreachable!("the root was just built");
        };
        let row = match tree.insert(
            body,
            floored_row_style(&scales),
            accesskit::Node::new(accesskit::Role::ListItem),
            WidgetKind::ListRow(ListRowState {
                selected: true,
                disabled: false,
            }),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(200.0, 200.0);

        let Some(row_bounds) = tree.bounds(row) else {
            unreachable!("just laid out");
        };
        assert_eq!(
            row_bounds.height, 21,
            "the row really does out-grow its 13px body -- that is the precondition: \
             {row_bounds:?}"
        );
        let theme = dark_theme();
        let (mesh, _) = single_paint(&tree, row, &theme, &scales, 1.0);
        let (_, top, _, bottom) = bbox(&mesh);
        assert!(
            top >= 0.0 && bottom <= 13.0,
            "a 21px row in a 13px clipping body must paint only inside it: {top} -> {bottom}"
        );
    }

    /// The same clip, taken to its end: a row laid out entirely past the
    /// bottom of its clipping body paints nothing at all. That makes
    /// paint agree with `WidgetTree::hit_test`, which already refuses to
    /// descend into a parent whose own bounds exclude the point.
    #[test]
    fn a_selected_list_row_past_the_bottom_of_its_body_paints_nothing() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let Ok(body) = insert_container(&mut tree, root, clipping_body(13.0)) else {
            unreachable!("the root was just built");
        };
        let mut rows = Vec::new();
        for _ in 0..3 {
            match tree.insert(
                body,
                floored_row_style(&scales),
                accesskit::Node::new(accesskit::Role::ListItem),
                WidgetKind::ListRow(ListRowState {
                    selected: true,
                    disabled: false,
                }),
            ) {
                Ok(id) => rows.push(id),
                Err(err) => unreachable!("{err:?}"),
            }
        }
        tree.compute_layout(200.0, 200.0);

        let Some(&last) = rows.last() else {
            unreachable!("three rows were just inserted");
        };
        let Some(last_bounds) = tree.bounds(last) else {
            unreachable!("just laid out");
        };
        assert!(
            last_bounds.y >= 13,
            "the third row must start past the 13px body -- the precondition: {last_bounds:?}"
        );
        let theme = dark_theme();
        let paints = match paint_widget(&tree, last, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            paints.is_empty(),
            "a row wholly outside its clipping body must paint nothing, the same \
             Ok(vec![]) an unselected row returns"
        );
    }

    /// The same guard for `TreeItem`, whose own
    /// `row_height(scales).min(bounds.height)` clamp never covered this:
    /// for a 21px row in a 13px body it computes `min(21, 21) = 21` and
    /// overhangs exactly as an unclamped `ListRow` would.
    #[test]
    fn a_selected_tree_rows_highlight_stays_inside_a_body_that_clips_its_overflow() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let Ok(body) = insert_container(&mut tree, root, clipping_body(13.0)) else {
            unreachable!("the root was just built");
        };
        let row = match insert_tree_item(&mut tree, body, &scales, "Squeezed", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_selected(&mut tree, row, true) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(200.0, 200.0);

        let Some(row_bounds) = tree.bounds(row) else {
            unreachable!("just laid out");
        };
        assert_eq!(
            row_bounds.height, 21,
            "the tree row out-grows its 13px body too: {row_bounds:?}"
        );
        let theme = dark_theme();
        let (mesh, _) = single_paint(&tree, row, &theme, &scales, 1.0);
        let (_, top, _, bottom) = bbox(&mesh);
        assert!(
            top >= 0.0 && bottom <= 13.0,
            "a tree row must be clipped to its own body as well: {top} -> {bottom}"
        );
    }

    /// A row squeezed shorter than one line must not paint outside its
    /// own bounds — what `.min(bounds.height)` is for.
    #[test]
    fn a_tree_rows_highlight_never_exceeds_its_own_bounds() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let row = match insert_tree_item(&mut tree, root, &scales, "Squeezed", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_selected(&mut tree, row, true) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.set_bounds(
            row,
            Rect {
                x: 0,
                y: 0,
                width: 200,
                height: 8,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (mesh, _) = single_paint(&tree, row, &theme, &scales, 1.0);
        let (_, top, _, bottom) = bbox(&mesh);
        assert!(
            top >= 0.0 && bottom <= 8.0,
            "an 8px-tall row's highlight must stay inside it: {top} -> {bottom}"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_laid_out_color_swatch_paints_its_own_arbitrary_color() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let color = Color {
            r: 12,
            g: 200,
            b: 90,
        };
        let swatch = match insert_color_swatch(&mut tree, root, &scales, color) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            swatch,
            Rect {
                x: 0,
                y: 0,
                width: 32,
                height: 32,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (mesh, paint_color) = single_paint(&tree, swatch, &theme, &scales, 1.0);
        assert!(
            !mesh.vertices.is_empty() && !mesh.indices.is_empty(),
            "a 32x32 color swatch must tessellate to real geometry"
        );
        let [r, g, b] = color.to_srgb_f32();
        assert_eq!(
            paint_color,
            [r, g, b, 1.0],
            "an enabled swatch must paint its own color, not a theme token, at full opacity"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_disabled_color_swatch_applies_the_theme_disabled_opacity() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let color = Color {
            r: 12,
            g: 200,
            b: 90,
        };
        let swatch = match insert_color_swatch(&mut tree, root, &scales, color) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_color_swatch_disabled(&mut tree, swatch, true) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let (_, paint_color) = single_paint(&tree, swatch, &theme, &scales, 1.0);
        let [r, g, b] = color.to_srgb_f32();
        assert_eq!(
            paint_color,
            [r, g, b, theme.state.disabled_opacity],
            "a disabled swatch still shows its own color, just dimmed"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_laid_out_panel_paints_surface_panel() {
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = scales();
        let panel = match tree.insert(
            root,
            taffy::Style::default(),
            accesskit::Node::new(accesskit::Role::Region),
            WidgetKind::Panel,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            panel,
            Rect {
                x: 0,
                y: 0,
                width: 240,
                height: 400,
            },
        ) {
            unreachable!("{err:?}");
        }
        let theme = dark_theme();

        let mut paints = match paint_widget(&tree, panel, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(paints.len(), 2, "a panel paints a fill and a border");
        let (border_mesh, border_color) = paints.remove(1);
        let (fill_mesh, fill_color) = paints.remove(0);
        assert!(
            !fill_mesh.vertices.is_empty() && !fill_mesh.indices.is_empty(),
            "a 240x400 panel's own fill must tessellate to real geometry"
        );
        assert!(
            !border_mesh.vertices.is_empty() && !border_mesh.indices.is_empty(),
            "a 240x400 panel's own border must tessellate to real geometry"
        );
        let [r, g, b] = theme.surface.panel.to_srgb_f32();
        assert_eq!(
            fill_color,
            [r, g, b, 1.0],
            "a panel's own background must use surface.panel at full opacity"
        );
        let [r, g, b] = theme.border.default.to_srgb_f32();
        assert_eq!(
            border_color,
            [r, g, b, 1.0],
            "a panel's own border must use border.default at full opacity"
        );
    }

    /// The window a dialog test lays out against. Any definite size
    /// works; this one matches `widgets::dialog`'s own layout tests so
    /// the two read as the same fixture.
    const DIALOG_WINDOW: (f32, f32) = (800.0, 600.0);

    /// A real, laid-out one-action dialog in a **definitely sized**
    /// root. The size is load-bearing, not incidental: `insert_dialog`'s
    /// own root style is `Position::Absolute` with a `percent(0.5)`
    /// width, so against `new_tree(Style::default())`'s auto-sized root
    /// the percentage has nothing to resolve against and the dialog
    /// silently collapses to its `min_size` floor -- a degenerate box
    /// that still paints, and would make every assertion below a
    /// statement about the wrong rectangle. Same idiom (and same
    /// reason) as `widgets::dialog`'s own `sized_tree`.
    fn laid_out_dialog(scales: &Scales) -> (WidgetTree<WidgetKind>, DialogHandle) {
        let (mut tree, root) = new_tree(taffy::Style {
            size: taffy::Size {
                width: taffy::style_helpers::length(DIALOG_WINDOW.0),
                height: taffy::style_helpers::length(DIALOG_WINDOW.1),
            },
            ..Default::default()
        });
        let handle = match insert_dialog(
            &mut tree,
            root,
            scales,
            "Aurora Didn't Close Properly",
            "The previous session didn't shut down cleanly.",
            vec![DialogAction::new("ok", "OK")],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(DIALOG_WINDOW.0, DIALOG_WINDOW.1);
        (tree, handle)
    }

    /// Resolves a dialog's own paint and asserts it is exactly the two
    /// shapes every non-High-Contrast theme produces -- a
    /// `surface.overlay` fill and the unconditional `border.default`
    /// outline over it -- returning both. The High Contrast case (a
    /// third, `border.control` shape on top) has its own test.
    fn dialog_fill_and_border(
        tree: &WidgetTree<WidgetKind>,
        handle: &DialogHandle,
        theme: &Theme,
        scales: &Scales,
    ) -> (Paint, Paint) {
        let mut paints = match paint_widget(tree, handle.root, theme, scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            paints.len(),
            2,
            "a dialog paints a fill and a border: {paints:?}"
        );
        let border = paints.remove(1);
        let fill = paints.remove(0);
        (fill, border)
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_laid_out_dialog_paints_surface_overlay_with_a_real_border_over_it() {
        let scales = scales();
        let (tree, handle) = laid_out_dialog(&scales);
        let theme = dark_theme();

        let ((fill_mesh, fill_color), (border_mesh, border_color)) =
            dialog_fill_and_border(&tree, &handle, &theme, &scales);
        assert!(
            !fill_mesh.vertices.is_empty() && !fill_mesh.indices.is_empty(),
            "a real, centred dialog box must tessellate to real geometry"
        );
        assert!(
            !border_mesh.vertices.is_empty() && !border_mesh.indices.is_empty(),
            "a dialog's own border must tessellate to real geometry too"
        );
        let [r, g, b] = theme.surface.overlay.to_srgb_f32();
        assert_eq!(
            fill_color,
            [r, g, b, 1.0],
            "a dialog's own surface must use surface.overlay at full opacity"
        );
        let [r, g, b] = theme.border.default.to_srgb_f32();
        assert_eq!(
            border_color,
            [r, g, b, 1.0],
            "a dialog's own border must use border.default at full opacity, the same \
             token paint_panel already strokes its own with"
        );
    }

    /// **The regression test for the Light-theme invisibility bug**
    /// found by review of `0.79.0` and fixed in `0.79.1`, stated against
    /// the exact widget pair that collides in the real app: a
    /// `WidgetKind::Dialog` over a `WidgetKind::Panel`.
    ///
    /// `design/themes/light.toml` resolves `surface.overlay`,
    /// `surface.panel`, `surface.raised` and `surface.canvas` all to
    /// `neutral.900` `#f5f5f6`, and sets `border.control_opacity = 0.0`
    /// so `control_outline` returns `None`. Through `0.79.0` a dialog's
    /// *entire* paint was therefore one `#f5f5f6` fill, byte-identical
    /// to the fill of the panel behind it at 1.000:1 -- reachable in the
    /// shipping app at `aurora-app`'s enforced 640x480 minimum window,
    /// where a real dialog does centre over real panel chrome. Nothing
    /// in this crate draws shadows, so there was nothing else to
    /// separate them.
    ///
    /// The first two assertions pin the collision itself rather than
    /// assuming it, so this test still says what it means if Light's
    /// tokens are later changed (it fails loudly rather than passing for
    /// a new reason). The claim is deliberately *not* "the dialog's
    /// whole paint list differs from the panel's" -- that is vacuous
    /// here, since the two lists differ in corner radius alone
    /// (`radius.md` vs `radius.sm`) and both borders come from the same
    /// `border.default` token. The real claim is that a dialog paints at
    /// least one colour its own backdrop does not, i.e. that its edge
    /// exists at all.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_light_theme_dialog_is_not_invisible_against_the_panel_behind_it() {
        let scales = scales();
        let theme = light_theme();

        assert_eq!(
            theme.surface.overlay, theme.surface.panel,
            "this test is only worth running while Light resolves a dialog's own fill \
             token and a panel's to the same value -- if they ever separate, the \
             invisibility this guards against is gone and this test needs rewriting"
        );
        assert_eq!(
            theme.border.control_opacity, 0.0,
            "... and while control_outline returns None in Light, which is what left \
             a dialog with no second shape at all through 0.79.0"
        );

        let (tree, handle) = laid_out_dialog(&scales);
        let paints = match paint_widget(&tree, handle.root, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        let [pr, pg, pb] = theme.surface.panel.to_srgb_f32();
        let backdrop = [pr, pg, pb, 1.0];
        assert!(
            paints.iter().any(|(_, color)| *color != backdrop),
            "in Light a dialog must paint at least one shape that is NOT the panel \
             colour behind it, or it is literally invisible over real panel chrome: \
             {paints:?}"
        );
        let Some((_, fill_color)) = paints.first() else {
            unreachable!("a dialog always paints at least its own fill");
        };
        assert_eq!(
            *fill_color, backdrop,
            "and the fill really is the colliding one -- it is the border, not the \
             fill, that is doing the work here"
        );
    }

    /// The one assertion that actually distinguishes this function's
    /// token choice from `paint_command_palette`'s -- and it can only be
    /// made in **two** of the five built-in themes.
    ///
    /// Light, High Contrast Dark and High Contrast Light each resolve
    /// `surface.overlay` and `surface.raised` to the *same* value
    /// (`neutral.900`, `hc.black`, `hc.white` respectively -- deliberate
    /// elevation choices in those files, not oversights), so the same
    /// assertion there would pass no matter which token `paint_dialog`
    /// read, i.e. it would be vacuous. Only Dark (`neutral.200` vs
    /// `neutral.300`) and Colour-Critical (`cc.raised` vs `cc.overlay`)
    /// separate them, so only those two are checked here -- and each is
    /// checked *starting* with an explicit `assert_ne!` on the two
    /// tokens, so if a future theme edit ever collapsed them this test
    /// fails loudly instead of quietly degrading into a tautology.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_dialog_paints_surface_overlay_not_the_command_palettes_surface_raised() {
        let scales = scales();
        for (name, theme) in [
            ("Dark", dark_theme()),
            ("Colour-Critical", color_critical_theme()),
        ] {
            let overlay = theme.surface.overlay.to_srgb_f32();
            let raised = theme.surface.raised.to_srgb_f32();
            assert_ne!(
                overlay, raised,
                "{name} is only worth testing because these two tokens differ in it -- \
                 if they ever collide here, this test has stopped proving anything and \
                 needs a theme that still separates them"
            );

            let (tree, handle) = laid_out_dialog(&scales);
            let ((_, color), _) = dialog_fill_and_border(&tree, &handle, &theme, &scales);
            let [r, g, b] = overlay;
            assert_eq!(
                color,
                [r, g, b, 1.0],
                "{name}: a dialog paints surface.overlay (Elevation 2: modals, dialogs)"
            );
            let [r, g, b] = raised;
            assert_ne!(
                color,
                [r, g, b, 1.0],
                "{name}: ... and specifically not surface.raised, which is what \
                 paint_command_palette reads (Elevation 1: popovers)"
            );
        }
    }

    /// A dialog carries the unconditional `border.default` outline
    /// [`paint_panel`] does *and* keeps the conditional `border.control`
    /// one, so a High Contrast theme gets three shapes, not two. That is
    /// deliberate: those two themes set `border.control` to pure
    /// white/black at full opacity ("mandatory strong borders on every
    /// control"), and collapsing to `border.default` alone -- an exact
    /// mirror of `paint_panel` -- would have downgraded them to
    /// `hc.mid_gray`. The third shape draws last, so it is what a user
    /// of those themes actually sees.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_dialog_gains_a_third_outline_shape_when_border_control_opacity_is_above_zero() {
        let scales = scales();
        let (tree, handle) = laid_out_dialog(&scales);
        let theme = high_contrast_theme();

        let paints = match paint_widget(&tree, handle.root, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            paints.len(),
            3,
            "a dialog paints its own surface, its border.default outline, and the \
             mandatory control outline over both: {paints:?}"
        );
        let Some((_, border_color)) = paints.get(1) else {
            unreachable!("just asserted len() == 3");
        };
        let [r, g, b] = theme.border.default.to_srgb_f32();
        assert_eq!(
            *border_color,
            [r, g, b, 1.0],
            "the unconditional border must still use border.default at full opacity"
        );
        let Some((_, outline_color)) = paints.get(2) else {
            unreachable!("just asserted len() == 3");
        };
        let [r, g, b] = theme.border.control.to_srgb_f32();
        assert_eq!(
            *outline_color,
            [r, g, b, theme.border.control_opacity],
            "the control outline must use border.control at border.control_opacity, \
             and must draw last so it lands on top of border.default"
        );
    }

    /// **The corner radius is this widget's one real geometric decision,
    /// and this is what pins it.** `paint_dialog` chooses
    /// `scales.radius.md` (`4`); review of `0.79.0` found that mutating
    /// it to every other value in the `radius` scale
    /// (`none`/`sm`/`lg`/`pill`) left every dialog test in this crate
    /// green, because none of them looked at the mesh's actual geometry
    /// and this widget deliberately ships no golden image.
    ///
    /// Two assertions, read straight off the fill mesh.
    /// `aurora_vector::rounded_rect` emits a real path anchor at
    /// `(x, y + r)` -- where the top-left arc rejoins the left edge --
    /// and `lyon`'s fill tessellator keeps every path endpoint as a
    /// vertex, so:
    ///
    /// 1. that exact point must be present, which is false for `sm`
    ///    (`2`), `lg` (`8`) and `pill` (clamped to half the box);
    /// 2. the square corner `(x, y)` must be *absent*, which is what
    ///    rules out `none` (`0`) -- the one mutation assertion 1 alone
    ///    would miss.
    ///
    /// Verified by actually performing all four mutations, not assumed.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_dialogs_fill_mesh_has_the_radius_md_corner_and_not_a_square_one() {
        let scales = scales();
        let (tree, handle) = laid_out_dialog(&scales);
        let theme = dark_theme();
        let Some(bounds) = tree.bounds(handle.root) else {
            unreachable!("just laid out");
        };

        let ((fill_mesh, _), _) = dialog_fill_and_border(&tree, &handle, &theme, &scales);
        #[allow(clippy::cast_precision_loss)]
        let (left, top, radius) = (bounds.x as f32, bounds.y as f32, scales.radius.md as f32);
        assert!(
            radius > 0.0,
            "the committed radius.md must be a real, non-zero radius: {radius}"
        );

        let has = |x: f32, y: f32| fill_mesh.vertices.iter().any(|v| v.x == x && v.y == y);
        assert!(
            has(left, top + radius),
            "the fill must carry rounded_rect's own (x, y + radius.md) anchor, which \
             pins the radius to exactly {radius}: {:?}",
            fill_mesh.vertices
        );
        assert!(
            !has(left, top),
            "... and must NOT carry the square top-left corner, which is what a \
             radius of 0 would produce: {:?}",
            fill_mesh.vertices
        );
    }

    /// Pins the "no text rendering" half of a dialog's scope: the
    /// message node holds the real message string for the
    /// accessibility tree and draws nothing at all. If this ever starts
    /// painting, either real text shaping landed (in which case this
    /// test should be rewritten around it) or a `WidgetKind` was
    /// changed by accident.
    #[test]
    fn a_dialogs_message_node_still_paints_nothing() {
        let scales = scales();
        let (tree, handle) = laid_out_dialog(&scales);
        let theme = dark_theme();

        let paints = match paint_widget(&tree, handle.message, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            paints.is_empty(),
            "a dialog's message is a plain Container -- this crate draws no glyphs, so \
             there is nothing to paint: {paints:?}"
        );
    }

    #[test]
    fn a_container_has_no_paint_yet() {
        let (tree, root) = new_tree(taffy::Style::default());
        let theme = dark_theme();
        let scales = scales();
        let paints = match paint_widget(&tree, root, &theme, &scales, 1.0) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            paints.is_empty(),
            "a plain Container has no paint defined yet"
        );
    }

    #[test]
    fn an_unknown_widget_id_is_an_error() {
        let (tree, _root) = new_tree(taffy::Style::default());
        let theme = dark_theme();
        let scales = scales();
        // Same bogus-id precedent `tree`'s own tests use
        // (`accesskit::NodeId(999)`) -- never inserted into this tree.
        let bogus = accesskit::NodeId(999);
        let result = paint_widget(&tree, bogus, &theme, &scales, 1.0);
        assert!(
            result.is_err(),
            "an id that was never inserted must not resolve"
        );
    }
}
