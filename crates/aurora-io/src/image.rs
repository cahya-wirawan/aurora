//! A decoded/to-be-encoded image: real pixel data plus the metadata
//! invariants §7.3.1b/§7.3.6 require — `f16` RGBA samples (promoted
//! immediately on import, never held as 8-bit) tagged with a real
//! colour space (never left untagged).
//!
//! Deliberately standalone, not wired into `aurora_doc::LayerTree`/
//! `aurora_tile::TileStore` yet: a layer doesn't own real pixel storage
//! yet either (see `aurora_doc::LayerKind`'s own doc comment on that
//! gap), so how an imported image becomes a document's actual layer
//! pixels is real, separate, still-open work. This type is this crate's
//! own, self-contained in-memory representation for round-tripping a
//! file — real for what it is used for today, not a placeholder for
//! something bigger that doesn't exist yet.

use aurora_color::IccProfile;
use half::f16;

use crate::error::IoError;

/// One RGBA image: `width * height * 4` `f16` samples, row-major
/// (matching `aurora_tile::Tile`'s own texel layout), tagged with its
/// own colour space.
#[derive(Debug)]
pub struct Image {
    width: u32,
    height: u32,
    color_space: IccProfile,
    samples: Vec<f16>,
}

impl Image {
    /// Builds an image from real, already-promoted samples.
    ///
    /// # Errors
    ///
    /// Returns [`IoError::SampleCountMismatch`] if `samples.len()` isn't
    /// exactly `width * height * 4`.
    pub fn new(
        width: u32,
        height: u32,
        color_space: IccProfile,
        samples: Vec<f16>,
    ) -> Result<Self, IoError> {
        let expected = expected_len(width, height);
        if samples.len() != expected {
            return Err(IoError::SampleCountMismatch {
                width,
                height,
                expected,
                actual: samples.len(),
            });
        }
        Ok(Self {
            width,
            height,
            color_space,
            samples,
        })
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn color_space(&self) -> &IccProfile {
        &self.color_space
    }

    /// `width * height * 4` `f16` samples, row-major RGBA.
    #[must_use]
    pub fn samples(&self) -> &[f16] {
        &self.samples
    }
}

fn expected_len(width: u32, height: u32) -> usize {
    width as usize * height as usize * 4
}

#[cfg(test)]
mod tests {
    use super::Image;
    use crate::error::IoError;
    use aurora_color::IccProfile;
    use half::f16;

    #[test]
    fn new_accepts_a_correctly_sized_buffer() {
        let samples = vec![f16::from_f32(0.0); 2 * 3 * 4];
        let image = match Image::new(2, 3, IccProfile::srgb(), samples) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(image.width(), 2);
        assert_eq!(image.height(), 3);
        assert_eq!(image.samples().len(), 24);
    }

    #[test]
    fn new_rejects_a_mismatched_buffer_length() {
        let samples = vec![f16::from_f32(0.0); 5];
        match Image::new(2, 3, IccProfile::srgb(), samples) {
            Err(IoError::SampleCountMismatch {
                width,
                height,
                expected,
                actual,
            }) => {
                assert_eq!(width, 2);
                assert_eq!(height, 3);
                assert_eq!(expected, 24);
                assert_eq!(actual, 5);
            }
            other => unreachable!("expected SampleCountMismatch, got {other:?}"),
        }
    }
}
