//! Palette parsing: the primitive colour ramps a theme's own tokens
//! reference by dotted path (`"neutral.100"`, `"accent.blue.500"`).

use toml::Value;

use crate::color::Color;
use crate::error::ThemeError;

/// A parsed `palette.toml`: nested ramp tables (`[neutral]`, one level
/// deep; `[accent.blue]`, two levels deep — the nesting depth genuinely
/// varies, which is why this holds a generic [`toml::Value`] tree rather
/// than fixed Rust structs, unlike [`crate::Theme`]'s fixed, known
/// vocabulary).
#[derive(Debug, Clone)]
pub struct Palette {
    root: Value,
}

impl Palette {
    /// Parses raw TOML source (e.g. the contents of `palette.toml`).
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError::Toml`] if `source` isn't valid TOML.
    pub fn from_toml_str(source: &str) -> Result<Self, ThemeError> {
        let root: Value = toml::from_str(source)?;
        Ok(Self { root })
    }

    /// Resolves a dotted reference (e.g. `"neutral.100"`,
    /// `"accent.blue.500"`) to a colour, by walking the ramp tables one
    /// path segment at a time down to a leaf hex string.
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError::UnknownPaletteReference`] if any path
    /// segment doesn't exist, [`ThemeError::WrongType`] if the reference
    /// leads to a table instead of a leaf value, or
    /// [`ThemeError::InvalidColor`] if the leaf isn't a valid
    /// `#RRGGBB` string.
    pub fn resolve(&self, reference: &str) -> Result<Color, ThemeError> {
        let mut node = &self.root;
        for segment in reference.split('.') {
            node = node
                .get(segment)
                .ok_or_else(|| ThemeError::UnknownPaletteReference(reference.to_owned()))?;
        }
        let hex = node.as_str().ok_or_else(|| ThemeError::WrongType {
            reference: reference.to_owned(),
            expected: "a color string leaf",
        })?;
        Color::from_hex(hex)
    }
}

#[cfg(test)]
mod tests {
    use super::Palette;
    use crate::ThemeError;

    // The real, committed, owner-approved palette (design/tokens/palette.toml)
    // -- parsing/resolving against real data, not a synthetic fixture.
    const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");

    fn palette() -> Palette {
        match Palette::from_toml_str(PALETTE_TOML) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn resolves_a_two_segment_ramp_reference() {
        let color = match palette().resolve("neutral.0") {
            Ok(c) => c,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(color.r, 0x14);
    }

    #[test]
    fn resolves_a_three_segment_nested_ramp_reference() {
        if let Err(err) = palette().resolve("accent.blue.500") {
            unreachable!("{err:?}");
        }
        if let Err(err) = palette().resolve("semantic.red.500") {
            unreachable!("{err:?}");
        }
    }

    #[test]
    fn rejects_an_unknown_path_segment() {
        match palette().resolve("neutral.9999") {
            Err(ThemeError::UnknownPaletteReference(got)) => assert_eq!(got, "neutral.9999"),
            other => unreachable!("expected UnknownPaletteReference, got {other:?}"),
        }
        match palette().resolve("not_a_real_ramp.0") {
            Err(ThemeError::UnknownPaletteReference(_)) => {}
            other => unreachable!("expected UnknownPaletteReference, got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_reference_that_lands_on_a_table_not_a_leaf() {
        match palette().resolve("neutral") {
            Err(ThemeError::WrongType { .. }) => {}
            other => unreachable!("expected WrongType, got {other:?}"),
        }
    }

    #[test]
    fn from_toml_str_rejects_invalid_toml() {
        match Palette::from_toml_str("this is not [ valid toml") {
            Err(ThemeError::Toml(_)) => {}
            other => unreachable!("expected Toml, got {other:?}"),
        }
    }
}
