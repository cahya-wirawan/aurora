//! An 8-bit sRGB color — what a palette ramp step and a resolved token
//! actually hold. Deliberately not `aurora_color::IccProfile`/`Transform`
//! territory: these are UI chrome colors from `design/tokens/palette.toml`
//! hex strings, not document/ICC-managed color.

use crate::error::ThemeError;

/// An 8-bit sRGB color, e.g. parsed from `"#RRGGBB"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    /// Parses `"#RRGGBB"` (case-insensitive hex digits).
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError::InvalidColor`] if `hex` isn't exactly
    /// `#` followed by 6 hex digits.
    pub fn from_hex(hex: &str) -> Result<Self, ThemeError> {
        let digits = hex
            .strip_prefix('#')
            .filter(|d| d.len() == 6)
            .ok_or_else(|| ThemeError::InvalidColor(hex.to_owned()))?;

        let byte = |i: usize| -> Result<u8, ThemeError> {
            digits
                .get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| ThemeError::InvalidColor(hex.to_owned()))
        };
        Ok(Self {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
        })
    }

    /// This color's three channels as `[0.0, 1.0]`-normalized `f32`
    /// samples, in sRGB gamma-encoded space (not linear — callers doing
    /// WCAG contrast math linearize via `aurora_color::srgb_to_linear`
    /// themselves, since that's a colour-science operation this crate
    /// only consumes, not owns).
    #[must_use]
    pub fn to_srgb_f32(self) -> [f32; 3] {
        [
            f32::from(self.r) / 255.0,
            f32::from(self.g) / 255.0,
            f32::from(self.b) / 255.0,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::Color;
    use crate::ThemeError;

    #[test]
    fn from_hex_parses_a_real_palette_value() {
        let color = match Color::from_hex("#141414") {
            Ok(c) => c,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            color,
            Color {
                r: 0x14,
                g: 0x14,
                b: 0x14
            }
        );
    }

    #[test]
    fn from_hex_is_case_insensitive() {
        let lower = Color::from_hex("#a4c8ff");
        let upper = Color::from_hex("#A4C8FF");
        match (lower, upper) {
            (Ok(a), Ok(b)) => assert_eq!(a, b),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn from_hex_rejects_missing_hash() {
        match Color::from_hex("141414") {
            Err(ThemeError::InvalidColor(got)) => assert_eq!(got, "141414"),
            other => unreachable!("expected InvalidColor, got {other:?}"),
        }
    }

    #[test]
    fn from_hex_rejects_wrong_length() {
        match Color::from_hex("#141") {
            Err(ThemeError::InvalidColor(_)) => {}
            other => unreachable!("expected InvalidColor, got {other:?}"),
        }
    }

    #[test]
    fn from_hex_rejects_non_hex_digits() {
        match Color::from_hex("#zzzzzz") {
            Err(ThemeError::InvalidColor(_)) => {}
            other => unreachable!("expected InvalidColor, got {other:?}"),
        }
    }

    #[test]
    // Exact, bit-representable results (0/255 = 0.0, 255/255 = 1.0), not
    // accumulated computation noise -- same reasoning `tree::tests`
    // already documents for its own float_cmp allows.
    #[allow(clippy::float_cmp)]
    fn to_srgb_f32_normalizes_the_full_byte_range() {
        assert_eq!(Color { r: 0, g: 0, b: 0 }.to_srgb_f32(), [0.0, 0.0, 0.0]);
        assert_eq!(
            Color {
                r: 255,
                g: 255,
                b: 255
            }
            .to_srgb_f32(),
            [1.0, 1.0, 1.0]
        );
    }
}
