//! Shared, theme-independent scales (`design/tokens/scales.toml`) — type,
//! spacing, radius, elevation, motion. Unlike [`crate::Theme`], these
//! have no palette references and no inheritance: every built-in theme
//! (Dark, Light, both high-contrast, Colour-Critical) uses the exact
//! same numbers, so this is a direct `serde` deserialization, not a
//! flatten-and-merge resolution.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct TypeSizeScale {
    pub xs: u32,
    pub sm: u32,
    pub md: u32,
    pub lg: u32,
    pub xl: u32,
    pub xxl: u32,
    pub display: u32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct TypeWeightScale {
    pub regular: u32,
    pub medium: u32,
    pub semibold: u32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct TypeLineHeightScale {
    pub tight: f32,
    pub normal: f32,
    pub relaxed: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TypeScale {
    /// Still an open placeholder in the design source itself
    /// (`design/tokens/scales.toml`: "Variable font family: TBD") — not
    /// blocking the rest of the token system, per that file's own
    /// comment.
    pub family: String,
    pub ratio: f32,
    pub size: TypeSizeScale,
    pub weight: TypeWeightScale,
    pub line_height: TypeLineHeightScale,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct DensityMultiplier {
    pub compact: f32,
    pub comfortable: f32,
    pub spacious: f32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct SpacingScale {
    pub base: u32,
    pub xxs: u32,
    pub xs: u32,
    pub sm: u32,
    pub md: u32,
    pub lg: u32,
    pub xl: u32,
    pub xxl: u32,
    pub xxxl: u32,
    pub density_multiplier: DensityMultiplier,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct RadiusScale {
    pub none: u32,
    pub sm: u32,
    pub md: u32,
    pub lg: u32,
    pub pill: u32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ElevationLevel {
    pub blur: u32,
    pub y: u32,
    pub alpha: f32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ElevationScale {
    #[serde(rename = "0")]
    pub level_0: ElevationLevel,
    #[serde(rename = "1")]
    pub level_1: ElevationLevel,
    #[serde(rename = "2")]
    pub level_2: ElevationLevel,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct MotionDuration {
    pub fast: u32,
    pub base: u32,
    pub slow: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MotionEasing {
    pub standard: String,
    pub entrance: String,
    pub exit: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MotionScale {
    pub duration: MotionDuration,
    pub easing: MotionEasing,
}

/// The full parsed contents of `scales.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Scales {
    #[serde(rename = "type")]
    pub typography: TypeScale,
    pub spacing: SpacingScale,
    pub radius: RadiusScale,
    pub elevation: ElevationScale,
    pub motion: MotionScale,
}

/// OS-level accessibility preferences a caller applies to a base
/// [`Scales`] via [`Scales::with_accessibility_preferences`] — distinct
/// from the density axis ([`SpacingScale::density_multiplier`], a
/// user's own explicit in-app choice) and distinct from theme selection
/// (`high_contrast` here only *records* the OS preference; switching to
/// an actual high-contrast theme needs one to exist first, still
/// Cahya's own design decision — see [`crate::Theme`]'s own doc
/// comment). Detecting the real values is platform-specific (real,
/// separate work per platform — `aurora-app` is the one crate allowed
/// to do that OS-facing detection, this crate stays plain data).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccessibilityPreferences {
    /// The OS asked UI motion be minimized. `with_accessibility_preferences`
    /// zeroes every `motion.duration` value under this preference, rather
    /// than leaving it for each future animation call site to re-check —
    /// a zero duration composes correctly with any code written against
    /// [`MotionDuration`] without that code needing its own
    /// reduced-motion branch.
    pub reduced_motion: bool,
    /// The OS' own high-contrast preference — see this struct's own doc
    /// comment for why nothing here acts on it yet.
    pub high_contrast: bool,
    /// A multiplier over `typography.size.*`; `1.0` means "no change."
    /// Not every platform exposes a real systemwide text-scale
    /// preference an application can read (macOS' accessibility-display
    /// API has no equivalent to iOS's Dynamic Type) — `1.0` is the
    /// honest default wherever real detection doesn't exist, not a
    /// stand-in for a value nobody could actually read.
    pub text_scale: f32,
}

impl Default for AccessibilityPreferences {
    fn default() -> Self {
        Self {
            reduced_motion: false,
            high_contrast: false,
            text_scale: 1.0,
        }
    }
}

impl Scales {
    /// Parses raw TOML source (e.g. the contents of `scales.toml`).
    ///
    /// # Errors
    ///
    /// Returns a `toml::de::Error` (via [`crate::ThemeError::Toml`]) if
    /// `source` isn't valid TOML matching this shape.
    pub fn from_toml_str(source: &str) -> Result<Self, crate::ThemeError> {
        Ok(toml::from_str(source)?)
    }

    /// A copy of `self` with `prefs` applied: `reduced_motion` zeroes
    /// every `motion.duration` value; `text_scale` multiplies every
    /// `typography.size` value, rounded to the nearest whole pixel (the
    /// same integer unit every other scale value already uses).
    /// `prefs.high_contrast` doesn't change anything returned here — see
    /// [`AccessibilityPreferences::high_contrast`]'s own doc comment.
    #[must_use]
    pub fn with_accessibility_preferences(&self, prefs: AccessibilityPreferences) -> Self {
        let mut scales = self.clone();
        if prefs.reduced_motion {
            scales.motion.duration = MotionDuration {
                fast: 0,
                base: 0,
                slow: 0,
            };
        }
        if (prefs.text_scale - 1.0).abs() > f32::EPSILON {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let scale = |value: u32| -> u32 { (value as f32 * prefs.text_scale).round() as u32 };
            let size = scales.typography.size;
            scales.typography.size = TypeSizeScale {
                xs: scale(size.xs),
                sm: scale(size.sm),
                md: scale(size.md),
                lg: scale(size.lg),
                xl: scale(size.xl),
                xxl: scale(size.xxl),
                display: scale(size.display),
            };
        }
        scales
    }
}

#[cfg(test)]
mod tests {
    use super::Scales;

    // The real, committed, owner-approved scales file.
    const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");

    #[test]
    fn parses_the_real_committed_scales_file() {
        let scales = match Scales::from_toml_str(SCALES_TOML) {
            Ok(s) => s,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(scales.typography.size.md, 13);
        assert_eq!(scales.spacing.base, 4);
        assert_eq!(scales.radius.pill, 9999);
        assert_eq!(scales.elevation.level_2.blur, 24);
        assert_eq!(scales.motion.duration.slow, 200);
        assert_eq!(
            scales.motion.easing.standard,
            "cubic-bezier(0.4, 0.0, 0.2, 1)"
        );
    }

    #[test]
    fn nothing_animates_longer_than_200ms() {
        // PRD FR-027's own stated ceiling -- a real, checkable invariant
        // on the committed scale, not just a doc comment.
        let scales = match Scales::from_toml_str(SCALES_TOML) {
            Ok(s) => s,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(scales.motion.duration.fast <= 200);
        assert!(scales.motion.duration.base <= 200);
        assert!(scales.motion.duration.slow <= 200);
    }

    #[test]
    fn comfortable_density_is_the_identity_multiplier() {
        let scales = match Scales::from_toml_str(SCALES_TOML) {
            Ok(s) => s,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!((scales.spacing.density_multiplier.comfortable - 1.0).abs() < 1e-6);
    }

    #[test]
    fn default_accessibility_preferences_are_the_identity() {
        let scales = match Scales::from_toml_str(SCALES_TOML) {
            Ok(s) => s,
            Err(err) => unreachable!("{err:?}"),
        };
        let adjusted =
            scales.with_accessibility_preferences(super::AccessibilityPreferences::default());
        assert_eq!(adjusted.motion.duration.slow, scales.motion.duration.slow);
        assert_eq!(adjusted.typography.size.md, scales.typography.size.md);
    }

    #[test]
    fn reduced_motion_zeroes_every_duration() {
        let scales = match Scales::from_toml_str(SCALES_TOML) {
            Ok(s) => s,
            Err(err) => unreachable!("{err:?}"),
        };
        let adjusted = scales.with_accessibility_preferences(super::AccessibilityPreferences {
            reduced_motion: true,
            ..Default::default()
        });
        assert_eq!(adjusted.motion.duration.fast, 0);
        assert_eq!(adjusted.motion.duration.base, 0);
        assert_eq!(adjusted.motion.duration.slow, 0);
        // Untouched by this preference.
        assert_eq!(adjusted.typography.size.md, scales.typography.size.md);
    }

    #[test]
    fn text_scale_multiplies_every_type_size() {
        let scales = match Scales::from_toml_str(SCALES_TOML) {
            Ok(s) => s,
            Err(err) => unreachable!("{err:?}"),
        };
        let adjusted = scales.with_accessibility_preferences(super::AccessibilityPreferences {
            text_scale: 2.0,
            ..Default::default()
        });
        assert_eq!(adjusted.typography.size.md, scales.typography.size.md * 2);
        assert_eq!(
            adjusted.typography.size.display,
            scales.typography.size.display * 2
        );
        // Untouched by this preference.
        assert_eq!(adjusted.motion.duration.slow, scales.motion.duration.slow);
    }

    #[test]
    fn high_contrast_alone_changes_nothing() {
        let scales = match Scales::from_toml_str(SCALES_TOML) {
            Ok(s) => s,
            Err(err) => unreachable!("{err:?}"),
        };
        let adjusted = scales.with_accessibility_preferences(super::AccessibilityPreferences {
            high_contrast: true,
            ..Default::default()
        });
        assert_eq!(adjusted.typography.size.md, scales.typography.size.md);
        assert_eq!(adjusted.motion.duration.slow, scales.motion.duration.slow);
    }
}
