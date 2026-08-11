//! Design tokens, theme parsing and inheritance, hot reload, contrast validation.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`Palette`]/[`Theme`]/[`ThemeSet`] parse `design/tokens/palette.toml`
//! and `design/themes/*.toml`, resolve `extends` inheritance, and produce
//! a fully-resolved [`Theme`] matching `design/tokens/vocabulary.md`'s
//! fixed token set. [`Scales`] parses the theme-independent
//! `design/tokens/scales.toml` (type/spacing/radius/elevation/motion).
//! [`contrast::check_gated_pairs`] is the real, CI-enforced version of
//! `design/check_contrast.py`'s Phase-0 prototype.
//! [`neutrality::check_surface_neutrality`] is the real, CI-enforced
//! satisfaction of PRD.md acceptance criterion 8 ("Colour-critical theme
//! surfaces are verified neutral").
//!
//! **Hot reload is not wired up yet** — [`ThemeSet::register`] can be
//! called again to re-parse and replace a theme, which is the half that
//! actually matters; watching the filesystem and calling it automatically
//! is thin, separate glue with no caller to drive it yet (`aurora-widgets`
//! is still a skeleton). **A CI lint rejecting hardcoded style values
//! (§7.3.10)** needs real widget code to lint against, which doesn't
//! exist yet either — both still open, see PLAN.md M1.6.
//!
//! **All five PRD-named built-in themes now exist as real, verified
//! designs** (owner-approved: Dark from PLAN 0.5, the other four each
//! delegated to engineering for implementation once Cahya made the colour
//! calls — see `design/themes/{dark,light,high-contrast-dark,
//! high-contrast-light,color-critical}.toml`). Dark/Light/High Contrast
//! Dark/High Contrast Light are each covered by a dedicated
//! `contrast::tests::the_real_{dark,light,high_contrast_dark,
//! high_contrast_light}_theme_passes_every_gated_pair` test.
//! Color-Critical is covered by both
//! `contrast::tests::the_real_color_critical_theme_passes_every_gated_pair`
//! (its 17 gated pairs, tighter margins than the two High Contrast themes
//! since this is a genuine mid-tone theme, not pure black/white — but
//! every pair still clears its floor with real margin) and
//! `neutrality::tests::the_real_color_critical_theme_has_neutral_surfaces`
//! (every checked token is exactly `r == g == b`, verified by construction
//! against a dedicated new `[cc]` palette table — the existing `[neutral]`
//! ramp carries a real, small, intentional cool tint on every step and
//! could not be reused). This closes out theme-file-level work for all
//! five PRD-named built-in themes; gallery/visual coverage for the three
//! newer ones (High Contrast Dark, High Contrast Light, Color-Critical)
//! remains separate, later, fully open work. Everything here is generic
//! over *any* correctly-shaped theme file.

pub mod contrast;
pub mod neutrality;

mod color;
mod error;
mod palette;
mod scales;
mod theme;

pub use color::Color;
pub use error::ThemeError;
pub use palette::Palette;
pub use scales::{
    AccessibilityPreferences, Density, DensityMultiplier, ElevationLevel, ElevationScale,
    MotionDuration, MotionEasing, MotionScale, RadiusScale, Scales, SpacingScale,
    TypeLineHeightScale, TypeScale, TypeSizeScale, TypeWeightScale,
};
pub use theme::{
    AccentTokens, BorderTokens, IconTokens, Overlay, StateTokens, SurfaceTokens, TextTokens, Theme,
    ThemeSet,
};
