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
}
