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
//!
//! **Hot reload is not wired up yet** — [`ThemeSet::register`] can be
//! called again to re-parse and replace a theme, which is the half that
//! actually matters; watching the filesystem and calling it automatically
//! is thin, separate glue with no caller to drive it yet (`aurora-widgets`
//! is still a skeleton). **A CI lint rejecting hardcoded style values
//! (§7.3.10)** needs real widget code to lint against, which doesn't
//! exist yet either — both still open, see PLAN.md M1.6.
//!
//! **Dark, Light, High Contrast Dark, and High Contrast Light exist as
//! real designs** (owner-approved: Dark from PLAN 0.5, the other three
//! each delegated to engineering for implementation once Cahya made the
//! colour calls — see `design/themes/{dark,light,high-contrast-dark,
//! high-contrast-light}.toml` and `contrast::tests`'
//! `the_real_{dark,light,high_contrast_dark,high_contrast_light}_theme_
//! passes_every_gated_pair`). Colour-Critical remains Cahya's one open
//! design decision to make (PRD FR-027 *Ownership*), not something this
//! crate invents while implementing the parser. Everything here is
//! generic over *any* correctly-shaped theme file.

pub mod contrast;

mod color;
mod error;
mod palette;
mod scales;
mod theme;

pub use color::Color;
pub use error::ThemeError;
pub use palette::Palette;
pub use scales::{
    AccessibilityPreferences, DensityMultiplier, ElevationLevel, ElevationScale, MotionDuration,
    MotionEasing, MotionScale, RadiusScale, Scales, SpacingScale, TypeLineHeightScale, TypeScale,
    TypeSizeScale, TypeWeightScale,
};
pub use theme::{
    AccentTokens, BorderTokens, IconTokens, Overlay, StateTokens, SurfaceTokens, TextTokens, Theme,
    ThemeSet,
};
