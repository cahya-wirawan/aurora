//! Resolves a widget's own layout bounds and state into the geometry
//! and colour [`crate::render::PathPipeline`]/[`crate::render::GpuMesh`]
//! need to actually draw it — the "wiring a widget's own paint through
//! this pipeline" step `render`'s own doc comment names as still open.
//!
//! **Scope, stated honestly.** [`paint_widget`] covers `Button`,
//! `Checkbox`, `Slider`, and `TextField` — solid rounded-rect shapes,
//! the simplest of the widgets this crate has (`widgets`' own doc
//! comment). `Checkbox`'s own box has no check/dash *glyph* drawn
//! inside it yet (this crate draws no glyphs at all — solid fills
//! only, `render`'s own doc comment); `Toggled::True` and
//! `Toggled::Mixed` currently render identically (both
//! `accent.primary`) since nothing yet exists to tell them apart
//! visually. `TextField` paints its own background only — no caret, no
//! selection highlight, no composition underline
//! (`composition_segments`' own byte-range *data* has no pixel
//! position to map to without real text shaping, which doesn't exist
//! in this crate; `content`/`cursor`/`selection_anchor`/`composition`
//! don't affect its paint at all today, only `disabled` does). Every
//! other [`WidgetKind`] returns `Ok(vec![])` (a real, deliberate
//! "nothing to paint yet"), not an error — `CommandPalette` still
//! needs its own shape (a list) designed and built the same way,
//! matching this project's own "no half-finished implementations"
//! practice rather than a fill-everything-with-a-rectangle
//! placeholder.
//!
//! [`paint_widget`] returns a `Vec<Paint>`, not a single `Paint` —
//! `Button`/`Checkbox` only ever needed one shape, but `Slider` is the
//! first widget that genuinely needs more than one (a track *and* a
//! thumb, different geometry, different colour, drawn in that order so
//! the thumb lands on top), and `CommandPalette`'s own future "a list"
//! shape will too (one row per visible command). Widening the return
//! type now, while there are only two real call sites
//! (`aurora-app::collect_widget_paints`,
//! `tests/gallery.rs::collect_gallery_paints`) to update, is cheaper
//! than doing it later after more of either exist.
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
//! does) — `scales.radius.sm`/`scales.radius.pill` are this function's
//! own reasonable choices, not a design decision made by Cahya (PRD
//! FR-027 *Ownership*); revisit if/when real per-widget radius tokens
//! are added.

use accesskit::Toggled;
use aurora_core::Rect;
use aurora_theme::{Scales, Theme};
use aurora_vector::{DEFAULT_TOLERANCE, Mesh, fill, rounded_rect};

use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};
use crate::widgets::{ButtonState, CheckboxState, SliderState, TextFieldState, WidgetKind};

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
        WidgetKind::Container | WidgetKind::CommandPalette(_) => Ok(vec![]),
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

#[cfg(test)]
mod tests {
    use super::{Paint, paint_widget};
    use crate::tree::{WidgetId, WidgetTree};
    use crate::widgets::{
        WidgetKind, insert_button, insert_checkbox, insert_slider, insert_text_field, new_tree,
        set_button_disabled, set_button_pressed, set_checkbox_disabled, set_slider_disabled,
        set_slider_value, set_text_field_disabled, toggle_checkbox,
    };
    use accesskit::Toggled;
    use aurora_core::Rect;
    use aurora_theme::{Palette, Scales, Theme, ThemeSet};

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
