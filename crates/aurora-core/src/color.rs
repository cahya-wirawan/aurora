//! Colour space and pixel format *descriptors*.
//!
//! These are tags, not storage — actual pixel buffers (and the `f16`/`f32`
//! arithmetic ADR 0003 specifies) live in `aurora-tile`/`aurora-graph`.
//! Keeping the split this way means `aurora-core` never needs a pixel-math
//! dependency (`half`, `bytemuck`, ...) of its own.

/// A colour space a buffer can be tagged with (invariant §7.3.6: every
/// buffer carries its colour space; untagged data is an error).
///
/// `#[non_exhaustive]`: `aurora-color` will need to add spaces (embedded
/// ICC profiles, for one) this crate has no business enumerating up front.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ColorSpace {
    LinearSrgb,
    Srgb,
    DisplayP3,
    AdobeRgb,
    ProPhotoRgb,
    Cmyk,
    Lab,
    Grayscale,
}

/// Channel layout of a pixel, independent of sample type or colour space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channels {
    Rgba,
    Rgb,
    GrayAlpha,
    Gray,
    Cmyk,
    CmykAlpha,
}

impl Channels {
    #[must_use]
    pub const fn count(self) -> u8 {
        match self {
            Self::Rgba | Self::Cmyk => 4,
            Self::Rgb | Self::CmykAlpha => 3,
            Self::GrayAlpha => 2,
            Self::Gray => 1,
        }
    }
}

/// Per-channel sample representation.
///
/// `U8` exists to describe import/export-boundary buffers only (invariant
/// §7.3.6b: no 8-bit intermediates in the graph). Nothing in this type
/// stops it being used elsewhere — that boundary is enforced where
/// buffers actually live (`aurora-tile`/`aurora-graph`), not here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    F16,
    F32,
    U8,
}

impl SampleFormat {
    #[must_use]
    pub const fn bytes(self) -> u8 {
        match self {
            Self::U8 => 1,
            Self::F16 => 2,
            Self::F32 => 4,
        }
    }
}

/// A complete pixel format descriptor: layout, sample type, and colour
/// space together, so a buffer's format and its colour tag can never be
/// passed around separately and drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelFormat {
    pub channels: Channels,
    pub sample: SampleFormat,
    pub color_space: ColorSpace,
}

impl PixelFormat {
    #[must_use]
    pub fn bytes_per_pixel(&self) -> u32 {
        u32::from(self.channels.count()) * u32::from(self.sample.bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::{Channels, ColorSpace, PixelFormat, SampleFormat};

    #[test]
    fn channel_counts() {
        assert_eq!(Channels::Rgba.count(), 4);
        assert_eq!(Channels::Rgb.count(), 3);
        assert_eq!(Channels::GrayAlpha.count(), 2);
        assert_eq!(Channels::Gray.count(), 1);
        assert_eq!(Channels::Cmyk.count(), 4);
        assert_eq!(Channels::CmykAlpha.count(), 3);
    }

    #[test]
    fn sample_bytes() {
        assert_eq!(SampleFormat::U8.bytes(), 1);
        assert_eq!(SampleFormat::F16.bytes(), 2);
        assert_eq!(SampleFormat::F32.bytes(), 4);
    }

    #[test]
    fn tile_format_matches_the_vertical_slice() {
        // spike/vertical-slice: RGBA f16 tiles, 8 bytes/px (spike/FINDINGS.md,
        // ADR 0003 "Storage: half-float RGBA tiles -- 8 bytes/px").
        let format = PixelFormat {
            channels: Channels::Rgba,
            sample: SampleFormat::F16,
            color_space: ColorSpace::LinearSrgb,
        };
        assert_eq!(format.bytes_per_pixel(), 8);
    }

    #[test]
    fn cmyk_format_byte_size() {
        let format = PixelFormat {
            channels: Channels::Cmyk,
            sample: SampleFormat::F32,
            color_space: ColorSpace::Cmyk,
        };
        assert_eq!(format.bytes_per_pixel(), 16);
    }
}
