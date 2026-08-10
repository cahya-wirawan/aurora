//! Chroma-neutrality validation over a resolved theme's tokens.
//!
//! This is the real, CI-enforced satisfaction of PRD.md's own acceptance
//! criterion 8: "Colour-critical theme surfaces are verified neutral
//! (chroma below an agreed threshold in a perceptual colour space)." This
//! crate's `Color` is an 8-bit sRGB triple, not a perceptual colour space,
//! so the concrete, verifiable form of "chroma below a threshold" this
//! module checks is the strict one the Colour-Critical theme's own design
//! was built to satisfy exactly: `r == g == b`. Any non-zero R/G/B spread
//! is by definition *some* hue tint (however small), so "exactly equal"
//! is a stronger, simpler guarantee than "chroma below an agreed
//! threshold" and implies it for any reasonable perceptual threshold.
//!
//! Shaped like `contrast.rs`'s `check_gated_pairs`: a list of labeled
//! checks, a result type, an `is_neutral`/`passes`-style predicate.

use crate::color::Color;
use crate::theme::Theme;

/// One neutrality check's result: whether `color` (a named token on a
/// resolved [`Theme`]) is genuinely chroma-neutral.
#[derive(Debug, Clone, Copy)]
pub struct NeutralityResult {
    pub label: &'static str,
    pub color: Color,
}

impl NeutralityResult {
    /// True iff `color` is exactly `r == g == b` -- no hue tint at all,
    /// not merely a small one.
    #[must_use]
    pub const fn is_neutral(&self) -> bool {
        self.color.r == self.color.g && self.color.g == self.color.b
    }
}

/// Checks every `SurfaceTokens` field (PRD.md acceptance criterion 8's
/// literal "surfaces") plus, going a bit further than the PRD's literal
/// wording, every `TextTokens`/`IconTokens`/`BorderTokens` field that the
/// Colour-Critical theme's own design also deliberately made neutral --
/// a stronger guarantee than PRD strictly requires, not something every
/// theme is held to. `border.focus` and `border.control` are excluded:
/// both legitimately reuse the accent colour (`accent.blue.600`, a real
/// hue) rather than a neutral gray, the same way every other built-in
/// theme's focus ring does, so they are not expected to pass this check.
#[must_use]
pub fn check_surface_neutrality(theme: &Theme) -> Vec<NeutralityResult> {
    vec![
        NeutralityResult {
            label: "surface.canvas",
            color: theme.surface.canvas,
        },
        NeutralityResult {
            label: "surface.app",
            color: theme.surface.app,
        },
        NeutralityResult {
            label: "surface.panel",
            color: theme.surface.panel,
        },
        NeutralityResult {
            label: "surface.raised",
            color: theme.surface.raised,
        },
        NeutralityResult {
            label: "surface.overlay",
            color: theme.surface.overlay,
        },
        NeutralityResult {
            label: "surface.sunken",
            color: theme.surface.sunken,
        },
        NeutralityResult {
            label: "text.primary",
            color: theme.text.primary,
        },
        NeutralityResult {
            label: "text.secondary",
            color: theme.text.secondary,
        },
        NeutralityResult {
            label: "text.disabled",
            color: theme.text.disabled,
        },
        NeutralityResult {
            label: "text.on_accent",
            color: theme.text.on_accent,
        },
        NeutralityResult {
            label: "icon.default",
            color: theme.icon.default,
        },
        NeutralityResult {
            label: "icon.secondary",
            color: theme.icon.secondary,
        },
        NeutralityResult {
            label: "icon.disabled",
            color: theme.icon.disabled,
        },
        NeutralityResult {
            label: "icon.on_accent",
            color: theme.icon.on_accent,
        },
        NeutralityResult {
            label: "border.default",
            color: theme.border.default,
        },
        NeutralityResult {
            label: "border.strong",
            color: theme.border.strong,
        },
        // border.focus / border.control deliberately excluded -- both
        // reuse the accent colour by design, same as every other theme.
    ]
}

#[cfg(test)]
mod tests {
    use super::check_surface_neutrality;
    use crate::color::Color;
    use crate::palette::Palette;
    use crate::theme::ThemeSet;

    const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
    const DARK_TOML: &str = include_str!("../../../design/themes/dark.toml");
    const COLOR_CRITICAL_TOML: &str = include_str!("../../../design/themes/color-critical.toml");

    #[test]
    fn a_non_neutral_color_fails_is_neutral() {
        // A deliberately tinted colour -- e.g. one lifted straight from
        // the existing [neutral] ramp, which this task's own brief
        // confirmed carries a small, real cool tint -- must be caught,
        // proving this check isn't vacuously true for everything.
        let tinted = super::NeutralityResult {
            label: "synthetic-tinted",
            color: Color {
                r: 0x40,
                g: 0x40,
                b: 0x48,
            },
        };
        assert!(
            !tinted.is_neutral(),
            "a colour with r != b must not report as neutral"
        );
    }

    #[test]
    fn a_genuinely_neutral_color_passes_is_neutral() {
        let neutral = super::NeutralityResult {
            label: "synthetic-neutral",
            color: Color {
                r: 0x54,
                g: 0x54,
                b: 0x54,
            },
        };
        assert!(neutral.is_neutral());
    }

    #[test]
    fn the_real_color_critical_theme_has_neutral_surfaces() {
        let palette = match Palette::from_toml_str(PALETTE_TOML) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut set = ThemeSet::new();
        // Color-Critical's `extends = "Dark"` needs the parent actually registered.
        if let Err(err) = set.register(DARK_TOML) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set.register(COLOR_CRITICAL_TOML) {
            unreachable!("{err:?}");
        }
        let theme = match set.resolve("Color-Critical", &palette) {
            Ok(t) => t,
            Err(err) => unreachable!("{err:?}"),
        };

        let results = check_surface_neutrality(&theme);
        assert_eq!(
            results.len(),
            16,
            "expected all 16 checked tokens (6 surface + 4 text + 4 icon + 2 border)"
        );
        for result in &results {
            assert!(
                result.is_neutral(),
                "{} is not chroma-neutral: r={} g={} b={}",
                result.label,
                result.color.r,
                result.color.g,
                result.color.b
            );
        }
    }

    #[test]
    fn the_real_dark_theme_does_not_claim_neutrality() {
        // Sanity cross-check: Dark's own surfaces come from [neutral],
        // which this task's own investigation confirmed carries a real
        // cool tint -- so this check should NOT report Dark's surfaces
        // as neutral. Proves the check has real discriminating power
        // rather than passing on every resolved theme regardless.
        let palette = match Palette::from_toml_str(PALETTE_TOML) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut set = ThemeSet::new();
        if let Err(err) = set.register(DARK_TOML) {
            unreachable!("{err:?}");
        }
        let theme = match set.resolve("Dark", &palette) {
            Ok(t) => t,
            Err(err) => unreachable!("{err:?}"),
        };

        let results = check_surface_neutrality(&theme);
        assert!(
            results.iter().any(|r| !r.is_neutral()),
            "Dark's [neutral]-sourced tokens were expected to include at least one \
             non-neutral (tinted) entry, proving this check has real discriminating power"
        );
    }
}
