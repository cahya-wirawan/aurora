use std::path::PathBuf;

/// Errors from [`crate::compare_to_golden`] and [`crate::Image`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TestkitError {
    /// `rgba`'s length isn't `width * height * 4`.
    #[error("rgba buffer has {actual_len} bytes, expected {expected_len} ({width}x{height} RGBA8)")]
    WrongBufferLength {
        width: u32,
        height: u32,
        expected_len: usize,
        actual_len: usize,
    },

    /// No golden image exists at `path` and bless mode
    /// (`AURORA_BLESS_GOLDEN`) wasn't set — the default, deliberately
    /// unhelpful-until-asked-for behaviour, so a first CI run can never
    /// silently establish an unreviewed baseline.
    #[error(
        "no golden image at {path}; re-run with AURORA_BLESS_GOLDEN=1 set to create it, \
         after reviewing the image it would produce"
    )]
    GoldenMissing { path: PathBuf },

    /// The golden file at `path` exists but isn't a decodable PNG.
    #[error("golden image at {path} is not a decodable PNG: {source}")]
    GoldenUndecodable {
        path: PathBuf,
        #[source]
        source: png::DecodingError,
    },

    /// The golden file at `path` decoded, but not as an 8-bit-per-channel
    /// RGBA image — e.g. a PNG placed there by hand from another tool,
    /// rather than one this crate's own `AURORA_BLESS_GOLDEN` wrote.
    /// Not `unreachable!()`: a golden a human authored externally is a
    /// real, foreseeable case, not an internal-invariant violation.
    #[error(
        "golden image at {path} decoded as {color_type:?}/{bit_depth:?}, not Rgba8 -- \
         re-save it as an 8-bit RGBA PNG, or re-bless it (AURORA_BLESS_GOLDEN=1)"
    )]
    GoldenWrongFormat {
        path: PathBuf,
        color_type: png::ColorType,
        bit_depth: png::BitDepth,
    },

    /// The golden image's dimensions don't match `actual`'s.
    #[error(
        "golden image at {path} is {golden_width}x{golden_height}, but the actual image is \
         {actual_width}x{actual_height}"
    )]
    DimensionMismatch {
        path: PathBuf,
        golden_width: u32,
        golden_height: u32,
        actual_width: u32,
        actual_height: u32,
    },

    /// At least one pixel channel differs from the golden by more than
    /// the caller's tolerance. `actual_path`/`diff_path` are written
    /// alongside `path` for manual review.
    #[error(
        "{mismatched_pixels} pixel(s) differ from the golden at {path} by more than \
         {tolerance} (max channel difference seen: {max_channel_diff}); wrote {actual_path} \
         and {diff_path} for review"
    )]
    PixelMismatch {
        path: PathBuf,
        actual_path: PathBuf,
        diff_path: PathBuf,
        tolerance: u8,
        max_channel_diff: u8,
        mismatched_pixels: usize,
    },

    /// Reading or writing a PNG (golden, actual, or diff) failed for a
    /// reason other than decoding — a real I/O failure, not a format
    /// mismatch.
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Encoding a PNG (golden, actual, or diff) for writing failed.
    #[error("failed to encode PNG at {path}: {source}")]
    Encoding {
        path: PathBuf,
        #[source]
        source: png::EncodingError,
    },
}
