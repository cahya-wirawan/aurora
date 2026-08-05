//! Errors from format import/export.

use thiserror::Error;

/// `#[non_exhaustive]`: more variants land as this crate grows past
/// PNG/JPEG into other formats.
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
    #[error("failed to decode JPEG: {0}")]
    JpegDecode(#[from] zune_jpeg::errors::DecodeErrors),
    #[error("failed to encode JPEG: {0}")]
    JpegEncode(#[from] jpeg_encoder::EncodingError),
    /// Requesting RGBA output from the JPEG decoder (see `jpeg` module
    /// doc comment) produced something else — the decoder's own docs
    /// warn it "does not guarantee... can convert to all colorspaces,"
    /// so this is a real, checked condition, not an assumption.
    #[error("decoded JPEG has an unexpected colour layout: {0:?}")]
    UnexpectedJpegColorSpace(zune_jpeg::zune_core::colorspace::ColorSpace),
    /// [`crate::jpeg::encode`] was given an [`crate::Image`] wider or
    /// taller than JPEG's own SOF marker can represent — a real,
    /// permanent format limit (16-bit dimension fields), not a library
    /// shortcoming.
    #[error("image is {width}x{height}, which exceeds JPEG's own 65535x65535 dimension limit")]
    JpegDimensionsTooLarge { width: u32, height: u32 },
    #[error("failed to decode TIFF: {0}")]
    TiffDecode(#[from] tiff::TiffError),
    #[error("failed to encode TIFF: {0}")]
    TiffEncode(tiff::TiffError),
    /// The decoded TIFF's own pixel layout (`tiff::ColorType`) is one
    /// this crate doesn't handle yet — see `tiff` module doc comment
    /// for exactly which layouts are covered (real, checked scope, not
    /// every TIFF variant that exists).
    #[error("decoded TIFF has an unsupported colour layout: {0:?}")]
    UnsupportedTiffColorType(tiff::ColorType),
    /// The decoded TIFF's own sample format (`tiff::decoder::DecodingResult`'s
    /// variant — e.g. 32-bit float) isn't one of the two this crate
    /// promotes from (8-bit or 16-bit unsigned integer samples).
    #[error("decoded TIFF uses an unsupported sample format: {0}")]
    UnsupportedTiffSampleFormat(&'static str),
    /// [`crate::Image::new`] was given a sample buffer whose length
    /// doesn't match `width * height * 4`.
    #[error("image is {width}x{height} (expects {expected} samples) but got {actual} samples")]
    SampleCountMismatch {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
    /// [`crate::import::write_into_store`] failed to page a touched tile
    /// in from the scratch disk.
    #[error("failed to write image into the tile store: {0}")]
    Tile(#[from] aurora_tile::TileError),
    /// [`crate::import::decode_by_extension`] was given a path whose
    /// extension names a format this crate doesn't decode (or has no
    /// extension at all) — real, checked scope, not every image format
    /// that exists.
    #[error("unsupported file extension: {0:?}")]
    UnsupportedExtension(String),
}
