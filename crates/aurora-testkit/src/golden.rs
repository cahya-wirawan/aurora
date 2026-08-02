use std::io::BufWriter;
use std::path::{Path, PathBuf};

use crate::error::TestkitError;
use crate::image::Image;

/// Set (to any value) to write `actual` as the golden image at `path`
/// instead of comparing against it — the deliberate, explicit way to
/// create or update a golden. Never read implicitly: a missing golden
/// is always [`TestkitError::GoldenMissing`] unless this is set, so a
/// first CI run can never silently establish an unreviewed baseline.
pub const BLESS_ENV_VAR: &str = "AURORA_BLESS_GOLDEN";

/// Compares `actual` against the golden `Rgba8` PNG at `path`.
///
/// Each pixel's per-channel absolute difference is checked against
/// `tolerance` — `0` requires a bit-exact match; a small nonzero value
/// tolerates the kind of numerical noise real GPU/driver differences
/// produce (the same class of noise `aurora-color`'s own lcms2 tests
/// already had to account for) without masking a real rendering
/// regression.
///
/// If [`BLESS_ENV_VAR`] is set, this always writes `actual` as the new
/// golden and returns `Ok(())` — it never compares in that mode. Otherwise:
/// a missing golden is [`TestkitError::GoldenMissing`]; a dimension
/// mismatch is [`TestkitError::DimensionMismatch`]; and a tolerance
/// violation is [`TestkitError::PixelMismatch`], having already written
/// `<path>` with its file stem suffixed `.actual.png` and `.diff.png`
/// (a white/black per-pixel mask: white wherever a pixel exceeded
/// `tolerance`) alongside the golden, for a human to open side by side.
///
/// # Errors
///
/// See [`TestkitError`].
pub fn compare_to_golden(path: &Path, actual: &Image, tolerance: u8) -> Result<(), TestkitError> {
    compare(
        path,
        actual,
        tolerance,
        std::env::var_os(BLESS_ENV_VAR).is_some(),
    )
}

/// [`compare_to_golden`]'s real logic, with bless mode as an explicit
/// parameter rather than read from the environment directly — so this
/// crate's own tests can exercise both modes deterministically without
/// mutating shared process state (`std::env::set_var` needs `unsafe` as
/// of the 2024 edition, precisely because of the cross-thread hazard
/// that would create between tests run in parallel; this sidesteps
/// needing it at all, here or anywhere else in this crate).
pub(crate) fn compare(
    path: &Path,
    actual: &Image,
    tolerance: u8,
    bless: bool,
) -> Result<(), TestkitError> {
    if bless {
        write_png(path, actual)?;
        tracing::info!(path = %path.display(), "blessed golden image");
        return Ok(());
    }

    if !path.exists() {
        return Err(TestkitError::GoldenMissing {
            path: path.to_path_buf(),
        });
    }
    let golden = read_png(path)?;
    if golden.width != actual.width || golden.height != actual.height {
        return Err(TestkitError::DimensionMismatch {
            path: path.to_path_buf(),
            golden_width: golden.width,
            golden_height: golden.height,
            actual_width: actual.width,
            actual_height: actual.height,
        });
    }

    let (max_channel_diff, mismatched_pixels, diff_rgba) =
        diff(&golden.rgba, &actual.rgba, tolerance);
    if mismatched_pixels == 0 {
        return Ok(());
    }

    let actual_path = sibling_path(path, "actual");
    let diff_path = sibling_path(path, "diff");
    write_png(&actual_path, actual)?;
    let diff_image = match Image::new(actual.width, actual.height, diff_rgba) {
        Ok(image) => image,
        Err(err) => {
            unreachable!("diff buffer is built at exactly actual's own length: {err}")
        }
    };
    write_png(&diff_path, &diff_image)?;

    Err(TestkitError::PixelMismatch {
        path: path.to_path_buf(),
        actual_path,
        diff_path,
        tolerance,
        max_channel_diff,
        mismatched_pixels,
    })
}

/// Returns `(max_channel_diff, mismatched_pixel_count, diff_mask_rgba)`.
/// `golden`/`actual` must already be confirmed the same length by the
/// caller (checked via matching `Image` dimensions).
fn diff(golden: &[u8], actual: &[u8], tolerance: u8) -> (u8, usize, Vec<u8>) {
    let mut max_channel_diff = 0u8;
    let mut mismatched_pixels = 0usize;
    let mut mask = vec![0u8; actual.len()];
    for ((golden_pixel, actual_pixel), mask_pixel) in golden
        .chunks_exact(4)
        .zip(actual.chunks_exact(4))
        .zip(mask.chunks_exact_mut(4))
    {
        let mut pixel_diff = 0u8;
        for (&g, &a) in golden_pixel.iter().zip(actual_pixel) {
            pixel_diff = pixel_diff.max(g.abs_diff(a));
        }
        max_channel_diff = max_channel_diff.max(pixel_diff);
        if pixel_diff > tolerance {
            mismatched_pixels += 1;
            mask_pixel.copy_from_slice(&[255, 255, 255, 255]);
        } else {
            mask_pixel.copy_from_slice(&[0, 0, 0, 255]);
        }
    }
    (max_channel_diff, mismatched_pixels, mask)
}

/// `<dir>/<stem>.<suffix>.png` — `path` with an extra suffix inserted
/// before its own extension, so `layer.png` becomes `layer.actual.png`/
/// `layer.diff.png` alongside it.
fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut result = path.to_path_buf();
    result.set_file_name(format!("{stem}.{suffix}.png"));
    result
}

fn write_png(path: &Path, image: &Image) -> Result<(), TestkitError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| TestkitError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    let file = std::fs::File::create(path).map_err(|source| TestkitError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|source| TestkitError::Encoding {
            path: path.to_path_buf(),
            source,
        })?;
    writer
        .write_image_data(&image.rgba)
        .map_err(|source| TestkitError::Encoding {
            path: path.to_path_buf(),
            source,
        })
}

fn read_png(path: &Path) -> Result<Image, TestkitError> {
    let file = std::fs::File::open(path).map_err(|source| TestkitError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let decoder = png::Decoder::new(file);
    let mut reader = decoder
        .read_info()
        .map_err(|source| TestkitError::GoldenUndecodable {
            path: path.to_path_buf(),
            source,
        })?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|source| TestkitError::GoldenUndecodable {
            path: path.to_path_buf(),
            source,
        })?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(TestkitError::GoldenWrongFormat {
            path: path.to_path_buf(),
            color_type: info.color_type,
            bit_depth: info.bit_depth,
        });
    }
    let rgba = match buf.get(..info.buffer_size()) {
        Some(bytes) => bytes.to_vec(),
        None => unreachable!("output_buffer_size sized buf to hold at least buffer_size bytes"),
    };
    match Image::new(info.width, info.height, rgba) {
        Ok(image) => Ok(image),
        Err(err) => {
            unreachable!("format was just confirmed Rgba8, so the buffer length must match: {err}")
        }
    }
}
