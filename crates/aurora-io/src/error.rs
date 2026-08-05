//! Errors from format import/export.

use thiserror::Error;

/// `#[non_exhaustive]`: more variants land as this crate grows past PNG
/// into other formats.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IoError {
    #[error("failed to decode PNG: {0}")]
    PngDecode(#[from] png::DecodingError),
    #[error("failed to encode PNG: {0}")]
    PngEncode(#[from] png::EncodingError),
    /// The PNG decoder's own `EXPAND`/`ALPHA` transformations were
    /// requested (see `png` module doc comment) but produced something
    /// other than RGBA — not expected for any real PNG this crate has
    /// seen, but a real, checked condition rather than an assumption:
    /// misreading a different channel layout as RGBA would silently
    /// scramble colour channels, not just fail loudly.
    #[error("decoded PNG has an unexpected colour layout: {0:?}")]
    UnexpectedColorType(png::ColorType),
    /// [`crate::Image::new`] was given a sample buffer whose length
    /// doesn't match `width * height * 4`.
    #[error("image is {width}x{height} (expects {expected} samples) but got {actual} samples")]
    SampleCountMismatch {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
}
