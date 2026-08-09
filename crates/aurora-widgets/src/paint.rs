//! Resolves a widget's own layout bounds and state into the geometry
//! and colour [`crate::render::PathPipeline`]/[`crate::render::GpuMesh`]
//! need to actually draw it — the "wiring a widget's own paint through
//! this pipeline" step `render`'s own doc comment names as still open.
//!
//! **Scope, stated honestly.** [`paint_widget`] covers `Button`,
//! `Checkbox`, `Slider`, `TextField`, `CommandPalette`, `ColorSwatch`,
//! `ListRow`, and `Panel` — solid rounded-rect shapes, the simplest of
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
//! returns. Every other [`WidgetKind`] (`Container` on its own) returns
//! `Ok(vec![])` too — a real, deliberate "nothing to paint," not an
//! error.
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

use accesskit::Toggled;
use aurora_core::Rect;
use aurora_theme::{Scales, Theme};
use aurora_vector::{DEFAULT_TOLERANCE, Mesh, fill, rounded_rect, stroke};

use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};
use crate::widgets::{
    ButtonState, CheckboxState, ColorSwatchState, ListRowState, SliderState, TextFieldState,
    WidgetKind,
};

/// One shape's own paint: tessellated fill geometry plus the straight,
/// unpremultiplied RGBA colour to draw it with — exactly the pair
/// [`crate::render::PathPipeline::bind_group`]/[`crate::render::
/// GpuMesh::upload`] need. A widget's *whole* paint is a `Vec<Paint>`
/// ([`paint_widget`]'s own return type) — see this module's own doc
/// comment for why a single widget can need more than one.
pub type Paint = (Mesh, [f32; 4]);

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
pub fn paint_widget(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
    theme: &Theme,
    scales: &Scales,
) -> Result<Vec<Paint>, WidgetError> {
    let bounds = tree.bounds(id).ok_or(WidgetError::UnknownWidget(id))?;
    let kind = tree.payload(id).ok_or(WidgetError::UnknownWidget(id))?;
    match kind {
        WidgetKind::Button(state) => paint_button(state, bounds, theme, scales).map(|p| vec![p]),
        WidgetKind::Checkbox(state) => {
            paint_checkbox(state, bounds, theme, scales).map(|p| vec![p])
        }
        WidgetKind::Slider(state) => paint_slider(state, bounds, theme, scales),
        WidgetKind::TextField(state) => {
            paint_text_field(state, bounds, theme, scales).map(|p| vec![p])
        }
        WidgetKind::CommandPalette(_) => {
            paint_command_palette(bounds, theme, scales).map(|p| vec![p])
        }
        WidgetKind::ColorSwatch(state) => {
            paint_color_swatch(*state, bounds, theme, scales).map(|p| vec![p])
        }
        WidgetKind::ListRow(state) => paint_list_row(*state, bounds, theme, scales),
        WidgetKind::Panel => paint_panel(bounds, theme, scales),
        WidgetKind::Container => Ok(vec![]),
    }
}

fn paint_button(
    state: &ButtonState,
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
) -> Result<Paint, WidgetError> {
    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.sm as f32,
    );
    let mesh = fill(&path, DEFAULT_TOLERANCE).map_err(WidgetError::Paint)?;

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
    Ok((mesh, [r, g, b, alpha]))
}

fn paint_checkbox(
    state: &CheckboxState,
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
) -> Result<Paint, WidgetError> {
    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.sm as f32,
    );
    let mesh = fill(&path, DEFAULT_TOLERANCE).map_err(WidgetError::Paint)?;

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
    Ok((mesh, [r, g, b, alpha]))
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
) -> Result<Vec<Paint>, WidgetError> {
    let alpha = if state.disabled {
        theme.state.disabled_opacity
    } else {
        1.0
    };

    let track_thickness = bounds.height as f32 * 0.3;
    let track_path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32 + (bounds.height as f32 - track_thickness) / 2.0,
        bounds.width as f32,
        track_thickness,
        scales.radius.pill as f32,
    );
    let track_mesh = fill(&track_path, DEFAULT_TOLERANCE).map_err(WidgetError::Paint)?;
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
    let thumb_mesh = fill(&thumb_path, DEFAULT_TOLERANCE).map_err(WidgetError::Paint)?;
    let [r, g, b] = theme.accent.primary.to_srgb_f32();
    let thumb = (thumb_mesh, [r, g, b, alpha]);

    Ok(vec![track, thumb])
}

fn paint_text_field(
    state: &TextFieldState,
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
) -> Result<Paint, WidgetError> {
    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.sm as f32,
    );
    let mesh = fill(&path, DEFAULT_TOLERANCE).map_err(WidgetError::Paint)?;

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
    Ok((mesh, [r, g, b, alpha]))
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
) -> Result<Paint, WidgetError> {
    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.md as f32,
    );
    let mesh = fill(&path, DEFAULT_TOLERANCE).map_err(WidgetError::Paint)?;
    let [r, g, b] = theme.surface.raised.to_srgb_f32();
    Ok((mesh, [r, g, b, 1.0]))
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
) -> Result<Paint, WidgetError> {
    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.sm as f32,
    );
    let mesh = fill(&path, DEFAULT_TOLERANCE).map_err(WidgetError::Paint)?;
    let [r, g, b] = state.color.to_srgb_f32();
    let alpha = if state.disabled {
        theme.state.disabled_opacity
    } else {
        1.0
    };
    Ok((mesh, [r, g, b, alpha]))
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
fn paint_list_row(
    state: ListRowState,
    bounds: Rect,
    theme: &Theme,
    scales: &Scales,
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
    let mesh = fill(&path, DEFAULT_TOLERANCE).map_err(WidgetError::Paint)?;
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
fn paint_panel(bounds: Rect, theme: &Theme, scales: &Scales) -> Result<Vec<Paint>, WidgetError> {
    const BORDER_WIDTH: f32 = 1.0;

    let path = rounded_rect(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        scales.radius.sm as f32,
    );
    let fill_mesh = fill(&path, DEFAULT_TOLERANCE).map_err(WidgetError::Paint)?;
    let [fr, fg, fb] = theme.surface.panel.to_srgb_f32();

    let border_mesh = stroke(&path, BORDER_WIDTH, DEFAULT_TOLERANCE).map_err(WidgetError::Paint)?;
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
        CommandEntry, ListRowState, WidgetKind, command_palette_state, insert_button,
        insert_checkbox, insert_color_swatch, insert_command_palette, insert_slider,
        insert_text_field, new_tree, set_button_disabled, set_button_pressed,
        set_checkbox_disabled, set_color_swatch_disabled, set_slider_disabled, set_slider_value,
        set_text_field_disabled, toggle_checkbox,
    };
    use accesskit::Toggled;
    use aurora_core::Rect;
    use aurora_theme::{Color, Palette, Scales, Theme, ThemeSet};

    const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
    const DARK_THEME_TOML: &str = include_str!("../../../design/themes/dark.toml");
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

    /// Resolves `id`'s own paint and asserts it's exactly one shape --
    /// `Button`/`Checkbox`'s own case -- returning that shape.
    fn single_paint(
        tree: &WidgetTree<WidgetKind>,
        id: WidgetId,
        theme: &Theme,
        scales: &Scales,
    ) -> Paint {
        let mut paints = match paint_widget(tree, id, theme, scales) {
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

        let (mesh, color) = single_paint(&tree, button, &theme, &scales);
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

        let (_, color) = single_paint(&tree, button, &theme, &scales);
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

        let (_, color) = single_paint(&tree, button, &theme, &scales);
        assert_eq!(color[3], theme.state.disabled_opacity);
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

        let (mesh, color) = single_paint(&tree, checkbox, &theme, &scales);
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

        let (_, color) = single_paint(&tree, checkbox, &theme, &scales);
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

        let (_, color) = single_paint(&tree, checkbox, &theme, &scales);
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

        let (_, color) = single_paint(&tree, checkbox, &theme, &scales);
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

        let mut paints = match paint_widget(&tree, slider, &theme, &scales) {
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
            let mut paints = match paint_widget(tree, slider, &theme, &scales) {
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

        let paints = match paint_widget(&tree, slider, &theme, &scales) {
            Ok(paints) => paints,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(paints.len(), 2);
        for (_, color) in &paints {
            assert_eq!(color[3], theme.state.disabled_opacity);
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

        let (mesh, color) = single_paint(&tree, field, &theme, &scales);
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

        let (_, color) = single_paint(&tree, field, &theme, &scales);
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

        let (mesh, color) = single_paint(&tree, palette, &theme, &scales);
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

        let (mesh, color) = single_paint(&tree, row, &theme, &scales);
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

        let paints = match paint_widget(&tree, second_row, &theme, &scales) {
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

        let (_, color) = single_paint(&tree, row, &theme, &scales);
        assert_eq!(
            color[3], theme.state.disabled_opacity,
            "a disabled, selected row still dims like every other disabled widget's paint"
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

        let (mesh, paint_color) = single_paint(&tree, swatch, &theme, &scales);
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

        let (_, paint_color) = single_paint(&tree, swatch, &theme, &scales);
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

        let mut paints = match paint_widget(&tree, panel, &theme, &scales) {
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

    #[test]
    fn a_container_has_no_paint_yet() {
        let (tree, root) = new_tree(taffy::Style::default());
        let theme = dark_theme();
        let scales = scales();
        let paints = match paint_widget(&tree, root, &theme, &scales) {
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
        let result = paint_widget(&tree, bogus, &theme, &scales);
        assert!(
            result.is_err(),
            "an id that was never inserted must not resolve"
        );
    }
}
