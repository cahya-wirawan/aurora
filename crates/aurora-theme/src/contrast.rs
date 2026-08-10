//! WCAG 2.1 contrast validation over a resolved theme's tokens.
//!
//! This is the real, CI-enforced version of what
//! `design/check_contrast.py` prototyped by hand during Phase 0 — that
//! script's own docstring already named this as the intended follow-on
//! ("not the CI lint described in PLAN.md M1.6 ... that one runs against
//! `aurora-theme`'s Rust token types"). Same formula, same gated pairs,
//! now a real `cargo test`.

use crate::color::Color;
use crate::theme::Theme;

/// A colour's WCAG relative luminance — reuses
/// `aurora_color::srgb_to_linear` for the channel linearization step,
/// since it's the exact same IEC 61966-2-1 curve either way.
fn relative_luminance(color: Color) -> f32 {
    let [r, g, b] = color.to_srgb_f32();
    let [r, g, b] = [
        aurora_color::srgb_to_linear(r),
        aurora_color::srgb_to_linear(g),
        aurora_color::srgb_to_linear(b),
    ];
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// The WCAG 2.1 contrast ratio between two colours, in `[1.0, 21.0]`.
#[must_use]
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (lighter, darker) = if la >= lb { (la, lb) } else { (lb, la) };
    (lighter + 0.05) / (darker + 0.05)
}

/// One token pair this crate gates in CI, and the WCAG floor it must
/// clear — the same list `design/check_contrast.py`'s own `PAIRS` used
/// during Phase 0 design review, carried forward as a real, automated
/// check over every built-in theme rather than a manually-run script.
struct GatedPair {
    label: &'static str,
    floor: f32,
    foreground: fn(&Theme) -> Color,
    background: fn(&Theme) -> Color,
}

const GATED_PAIRS: &[GatedPair] = &[
    GatedPair {
        label: "text.primary on surface.canvas",
        floor: 4.5,
        foreground: |t| t.text.primary,
        background: |t| t.surface.canvas,
    },
    GatedPair {
        label: "text.primary on surface.app",
        floor: 4.5,
        foreground: |t| t.text.primary,
        background: |t| t.surface.app,
    },
    GatedPair {
        label: "text.primary on surface.panel",
        floor: 4.5,
        foreground: |t| t.text.primary,
        background: |t| t.surface.panel,
    },
    GatedPair {
        label: "text.primary on surface.raised",
        floor: 4.5,
        foreground: |t| t.text.primary,
        background: |t| t.surface.raised,
    },
    GatedPair {
        label: "text.primary on surface.overlay",
        floor: 4.5,
        foreground: |t| t.text.primary,
        background: |t| t.surface.overlay,
    },
    GatedPair {
        label: "text.primary on surface.sunken",
        floor: 4.5,
        foreground: |t| t.text.primary,
        background: |t| t.surface.sunken,
    },
    GatedPair {
        label: "text.secondary on surface.panel",
        floor: 4.5,
        foreground: |t| t.text.secondary,
        background: |t| t.surface.panel,
    },
    GatedPair {
        label: "text.secondary on surface.raised",
        floor: 4.5,
        foreground: |t| t.text.secondary,
        background: |t| t.surface.raised,
    },
    GatedPair {
        label: "text.secondary on surface.canvas",
        floor: 4.5,
        foreground: |t| t.text.secondary,
        background: |t| t.surface.canvas,
    },
    GatedPair {
        label: "text.on_accent on accent.primary",
        floor: 4.5,
        foreground: |t| t.text.on_accent,
        background: |t| t.accent.primary,
    },
    GatedPair {
        label: "text.on_accent on accent.primary_hover",
        floor: 4.5,
        foreground: |t| t.text.on_accent,
        background: |t| t.accent.primary_hover,
    },
    GatedPair {
        label: "border.strong on surface.panel",
        floor: 3.0,
        foreground: |t| t.border.strong,
        background: |t| t.surface.panel,
    },
    GatedPair {
        label: "border.strong on surface.app",
        floor: 3.0,
        foreground: |t| t.border.strong,
        background: |t| t.surface.app,
    },
    GatedPair {
        label: "border.focus on surface.panel",
        floor: 3.0,
        foreground: |t| t.border.focus,
        background: |t| t.surface.panel,
    },
    GatedPair {
        label: "border.focus on surface.canvas",
        floor: 3.0,
        foreground: |t| t.border.focus,
        background: |t| t.surface.canvas,
    },
    GatedPair {
        label: "border.focus on surface.raised",
        floor: 3.0,
        foreground: |t| t.border.focus,
        background: |t| t.surface.raised,
    },
    GatedPair {
        label: "accent.primary on surface.panel",
        floor: 3.0,
        foreground: |t| t.accent.primary,
        background: |t| t.surface.panel,
    },
];

/// One gated pair's result: whether `theme` clears the WCAG floor for it.
#[derive(Debug, Clone, Copy)]
pub struct ContrastResult {
    pub label: &'static str,
    pub ratio: f32,
    pub floor: f32,
}

impl ContrastResult {
    #[must_use]
    pub const fn passes(&self) -> bool {
        self.ratio >= self.floor
    }
}

/// Checks every gated pair (`design/check_contrast.py`'s own `PAIRS`)
/// against a resolved theme.
#[must_use]
pub fn check_gated_pairs(theme: &Theme) -> Vec<ContrastResult> {
    GATED_PAIRS
        .iter()
        .map(|pair| ContrastResult {
            label: pair.label,
            ratio: contrast_ratio((pair.foreground)(theme), (pair.background)(theme)),
            floor: pair.floor,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{check_gated_pairs, contrast_ratio};
    use crate::color::Color;
    use crate::palette::Palette;
    use crate::theme::ThemeSet;

    const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
    const DARK_TOML: &str = include_str!("../../../design/themes/dark.toml");
    const LIGHT_TOML: &str = include_str!("../../../design/themes/light.toml");
    const HIGH_CONTRAST_DARK_TOML: &str =
        include_str!("../../../design/themes/high-contrast-dark.toml");
    const HIGH_CONTRAST_LIGHT_TOML: &str =
        include_str!("../../../design/themes/high-contrast-light.toml");

    #[test]
    fn identical_colors_have_a_ratio_of_one() {
        let black = Color { r: 0, g: 0, b: 0 };
        assert!((contrast_ratio(black, black) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn black_on_white_is_the_maximum_ratio() {
        let black = Color { r: 0, g: 0, b: 0 };
        let white = Color {
            r: 255,
            g: 255,
            b: 255,
        };
        let ratio = contrast_ratio(black, white);
        assert!(
            (ratio - 21.0).abs() < 0.1,
            "expected ~21:1 for black on white, got {ratio}"
        );
    }

    #[test]
    fn contrast_ratio_is_symmetric() {
        let a = Color {
            r: 20,
            g: 20,
            b: 20,
        };
        let b = Color {
            r: 200,
            g: 200,
            b: 200,
        };
        assert!((contrast_ratio(a, b) - contrast_ratio(b, a)).abs() < 1e-4);
    }

    #[test]
    fn the_real_dark_theme_passes_every_gated_pair() {
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

        let results = check_gated_pairs(&theme);
        assert_eq!(
            results.len(),
            17,
            "expected all 17 gated pairs to be checked"
        );
        for result in &results {
            assert!(
                result.passes(),
                "{} failed: {:.2}:1 against a {:.1}:1 floor",
                result.label,
                result.ratio,
                result.floor
            );
        }
    }

    #[test]
    fn the_real_light_theme_passes_every_gated_pair() {
        let palette = match Palette::from_toml_str(PALETTE_TOML) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut set = ThemeSet::new();
        // Light's `extends = "Dark"` needs the parent actually registered.
        if let Err(err) = set.register(DARK_TOML) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set.register(LIGHT_TOML) {
            unreachable!("{err:?}");
        }
        let theme = match set.resolve("Light", &palette) {
            Ok(t) => t,
            Err(err) => unreachable!("{err:?}"),
        };

        let results = check_gated_pairs(&theme);
        assert_eq!(
            results.len(),
            17,
            "expected all 17 gated pairs to be checked"
        );
        for result in &results {
            assert!(
                result.passes(),
                "{} failed: {:.2}:1 against a {:.1}:1 floor",
                result.label,
                result.ratio,
                result.floor
            );
        }
    }

    #[test]
    fn the_real_high_contrast_dark_theme_passes_every_gated_pair() {
        let palette = match Palette::from_toml_str(PALETTE_TOML) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut set = ThemeSet::new();
        // High Contrast Dark's `extends = "Dark"` needs the parent actually registered.
        if let Err(err) = set.register(DARK_TOML) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set.register(HIGH_CONTRAST_DARK_TOML) {
            unreachable!("{err:?}");
        }
        let theme = match set.resolve("High Contrast Dark", &palette) {
            Ok(t) => t,
            Err(err) => unreachable!("{err:?}"),
        };

        let results = check_gated_pairs(&theme);
        assert_eq!(
            results.len(),
            17,
            "expected all 17 gated pairs to be checked"
        );
        for result in &results {
            assert!(
                result.passes(),
                "{} failed: {:.2}:1 against a {:.1}:1 floor",
                result.label,
                result.ratio,
                result.floor
            );
        }
    }

    #[test]
    fn the_real_high_contrast_light_theme_passes_every_gated_pair() {
        let palette = match Palette::from_toml_str(PALETTE_TOML) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut set = ThemeSet::new();
        // High Contrast Light's `extends = "Dark"` needs the parent actually registered.
        if let Err(err) = set.register(DARK_TOML) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set.register(HIGH_CONTRAST_LIGHT_TOML) {
            unreachable!("{err:?}");
        }
        let theme = match set.resolve("High Contrast Light", &palette) {
            Ok(t) => t,
            Err(err) => unreachable!("{err:?}"),
        };

        let results = check_gated_pairs(&theme);
        assert_eq!(
            results.len(),
            17,
            "expected all 17 gated pairs to be checked"
        );
        for result in &results {
            assert!(
                result.passes(),
                "{} failed: {:.2}:1 against a {:.1}:1 floor",
                result.label,
                result.ratio,
                result.floor
            );
        }
    }
}
