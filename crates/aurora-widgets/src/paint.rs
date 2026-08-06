//! Resolves a widget's own layout bounds and state into the geometry
//! and colour [`crate::render::PathPipeline`]/[`crate::render::GpuMesh`]
//! need to actually draw it — the "wiring a widget's own paint through
//! this pipeline" step `render`'s own doc comment names as still open.
//!
//! **Scope, stated honestly.** [`paint_widget`] covers `Button` only —
//! a solid, rounded-rect background, the simplest of the widgets this
//! crate has (`widgets`' own doc comment). Every other [`WidgetKind`]
//! returns `Ok(None)` (a real, deliberate "nothing to paint yet"), not
//! an error — `Checkbox`/`Slider`/`TextField`/`CommandPalette` all need
//! their own shapes (a check glyph, a track + thumb, a caret/selection,
//! a list) designed and built the same way, one at a time, matching
//! this project's own "no half-finished implementations" practice
//! rather than a fill-everything-with-a-rectangle placeholder.
//!
//! Colour always comes from a real, resolved [`Theme`] token
//! (`accent.primary`/`accent.primary_active`, `state.disabled_opacity`)
//! — invariant §7.3.10, never a literal. The returned `[f32; 4]` is
//! straight (unpremultiplied) sRGB-gamma-encoded RGBA, [`Color::
//! to_srgb_f32`]'s own convention — matching what [`crate::render::
//! PathPipeline::bind_group`]'s own doc comment expects, but *not* yet
//! linearized for an sRGB-aware render target the way `aurora-app`'s
//! own `load_background_color` linearizes `surface.app` before using it
//! as a clear colour. Whether the real render target this eventually
//! draws into is sRGB-aware is an `aurora-app` integration decision
//! that doesn't exist yet; this function's job stops at "the token's
//! own colour, resolved," the same seam `load_background_color` itself
//! sits on the other side of.
//!
//! No button corner-radius token exists in `design/tokens/vocabulary.md`
//! yet (only the bare `radius.*` scale does) — `scales.radius.sm` is
//! this function's own reasonable choice, not a design decision made by
//! Cahya (PRD FR-027 *Ownership*); revisit if/when a real per-widget
//! radius token is added.

use aurora_core::Rect;
use aurora_theme::{Scales, Theme};
use aurora_vector::{DEFAULT_TOLERANCE, Mesh, fill, rounded_rect};

use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};
use crate::widgets::{ButtonState, WidgetKind};

/// A widget's own paint: tessellated fill geometry plus the straight,
/// unpremultiplied RGBA colour to draw it with — exactly the pair
/// [`crate::render::PathPipeline::bind_group`]/[`crate::render::
/// GpuMesh::upload`] need.
pub type Paint = (Mesh, [f32; 4]);

/// Resolves `id`'s own paint from its current layout bounds
/// ([`WidgetTree::bounds`]) and state, or `Ok(None)` if this
/// [`WidgetKind`] has no paint defined yet (see this module's own doc
/// comment for exactly which do).
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
) -> Result<Option<Paint>, WidgetError> {
    let bounds = tree.bounds(id).ok_or(WidgetError::UnknownWidget(id))?;
    let kind = tree.payload(id).ok_or(WidgetError::UnknownWidget(id))?;
    match kind {
        WidgetKind::Button(state) => paint_button(state, bounds, theme, scales).map(Some),
        WidgetKind::Container
        | WidgetKind::Checkbox(_)
        | WidgetKind::Slider(_)
        | WidgetKind::TextField(_)
        | WidgetKind::CommandPalette(_) => Ok(None),
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

#[cfg(test)]
mod tests {
    use super::paint_widget;
    use crate::widgets::{insert_button, new_tree, set_button_disabled, set_button_pressed};
    use aurora_core::Rect;
    use aurora_theme::{Palette, Scales, ThemeSet};

    const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
    const DARK_THEME_TOML: &str = include_str!("../../../design/themes/dark.toml");
    const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");

    fn dark_theme() -> aurora_theme::Theme {
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

        let paint = match paint_widget(&tree, button, &theme, &scales) {
            Ok(Some(paint)) => paint,
            other => unreachable!("a laid-out button must paint something: {other:?}"),
        };
        let (mesh, color) = paint;
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

        let (_, color) = match paint_widget(&tree, button, &theme, &scales) {
            Ok(Some(paint)) => paint,
            other => unreachable!("{other:?}"),
        };
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

        let (_, color) = match paint_widget(&tree, button, &theme, &scales) {
            Ok(Some(paint)) => paint,
            other => unreachable!("{other:?}"),
        };
        assert_eq!(color[3], theme.state.disabled_opacity);
    }

    #[test]
    fn a_container_has_no_paint_yet() {
        let (tree, root) = new_tree(taffy::Style::default());
        let theme = dark_theme();
        let scales = scales();
        let paint = match paint_widget(&tree, root, &theme, &scales) {
            Ok(paint) => paint,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(paint, None, "a plain Container has no paint defined yet");
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
