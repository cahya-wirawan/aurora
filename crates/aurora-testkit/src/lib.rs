//! Golden-image diff harness for render-correctness regression tests
//! (PRD §8.5, §13 Step 5; PLAN.md 0.2 — "needed before the first
//! filter").
//!
//! **Dev-dependency only.** This crate is never a real dependency of
//! shipped code — every consumer (`aurora-render` today; `aurora-filters`
//! and the `aurora-widgets` component gallery once those exist) reaches
//! it via `[dev-dependencies]`. That's also why it's deliberately
//! dependency-free of the rest of the workspace (`scripts/layering.json`
//! lists it with no allowed internal deps of its own): its two known
//! future consumers sit in different branches of the layering tree
//! (`aurora-widgets` cannot depend on `aurora-render`), so anything this
//! crate needed from elsewhere in the workspace would have to already
//! live at the very bottom of the stack. It doesn't, today — this crate
//! works entirely in already-quantized `Rgba8` bytes; converting a
//! pipeline's own precision (`f16` tiles, for instance) down to that is
//! each caller's job, not this crate's.
//!
//! [`Image`] is the in-memory `Rgba8` image type every golden/actual/diff
//! PNG uses. [`compare_to_golden`] does the real work: compares `actual`
//! against a golden PNG on disk within a per-channel `tolerance` (GPU/
//! driver differences mean real renders are rarely bit-exact even when
//! correct), and — on a mismatch — writes the actual image and a visual
//! diff mask alongside the golden for a human to review, rather than
//! just reporting a number. [`BLESS_ENV_VAR`] (`AURORA_BLESS_GOLDEN`) is
//! the explicit, deliberate way to create or update a golden; a golden
//! is never established implicitly, so a first CI run can't silently
//! accept an unreviewed baseline.

mod error;
mod golden;
mod image;

pub use error::TestkitError;
pub use golden::{BLESS_ENV_VAR, compare_to_golden};
pub use image::Image;

#[cfg(test)]
mod tests {
    use super::golden::compare;
    use super::{Image, TestkitError};
    use std::path::PathBuf;

    /// A tiny, distinctive image: a red pixel, a green pixel, a blue
    /// pixel, and a light-grey pixel, laid out 2x2 -- big enough to
    /// exercise real per-pixel diffing without a real render. Deliberately
    /// not pure white/black in any channel: `nudge` below adds a delta via
    /// `saturating_add`, and a channel already at 255 (or, for a
    /// subtracting nudge, 0) would silently clip to no real change,
    /// undercounting how many pixels a test expects to differ.
    fn sample() -> Image {
        let pixels: [[u8; 4]; 4] = [
            [200, 0, 0, 255],
            [0, 200, 0, 255],
            [0, 0, 200, 255],
            [200, 200, 200, 255],
        ];
        let rgba = pixels.into_iter().flatten().collect();
        match Image::new(2, 2, rgba) {
            Ok(image) => image,
            Err(err) => unreachable!("4 pixels at 2x2 is exactly width*height*4 bytes: {err}"),
        }
    }

    fn nudge(image: &Image, delta: u8) -> Image {
        let rgba = image
            .rgba
            .iter()
            .map(|&b| b.saturating_add(delta))
            .collect();
        match Image::new(image.width, image.height, rgba) {
            Ok(image) => image,
            Err(err) => unreachable!("same dimensions as the source image: {err}"),
        }
    }

    /// A fresh path inside a per-test tempdir, so tests never share or
    /// leave behind real files.
    fn golden_path() -> (tempfile::TempDir, PathBuf) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
        };
        let path = dir.path().join("golden.png");
        (dir, path)
    }

    fn bless(path: &std::path::Path, image: &Image) {
        if let Err(err) = compare(path, image, 0, true) {
            unreachable!("bless mode must always succeed: {err}");
        }
    }

    #[test]
    fn image_new_rejects_a_wrong_length_buffer() {
        match Image::new(2, 2, vec![0; 10]) {
            Err(TestkitError::WrongBufferLength {
                width: 2,
                height: 2,
                expected_len: 16,
                actual_len: 10,
            }) => {}
            other => unreachable!("expected WrongBufferLength, got {other:?}"),
        }
    }

    #[test]
    fn missing_golden_is_an_error_without_bless_mode() {
        let (_dir, path) = golden_path();
        match compare(&path, &sample(), 0, false) {
            Err(TestkitError::GoldenMissing { path: got }) => assert_eq!(got, path),
            other => unreachable!("expected GoldenMissing, got {other:?}"),
        }
    }

    #[test]
    fn bless_mode_writes_the_golden_and_always_succeeds() {
        let (_dir, path) = golden_path();
        bless(&path, &sample());
        assert!(path.exists());
    }

    #[test]
    fn an_identical_image_matches_a_blessed_golden() {
        let (_dir, path) = golden_path();
        bless(&path, &sample());

        if let Err(err) = compare(&path, &sample(), 0, false) {
            unreachable!("an unmodified round trip through PNG must match bit-exactly: {err}");
        }
    }

    #[test]
    fn a_small_difference_within_tolerance_still_matches() {
        let (_dir, path) = golden_path();
        bless(&path, &sample());

        let nudged = nudge(&sample(), 2);
        if let Err(err) = compare(&path, &nudged, 5, false) {
            unreachable!("a diff of 2 must be within a tolerance of 5: {err}");
        }
    }

    #[test]
    fn a_difference_beyond_tolerance_fails_and_writes_review_artifacts() {
        let (_dir, path) = golden_path();
        bless(&path, &sample());

        let nudged = nudge(&sample(), 50);
        match compare(&path, &nudged, 5, false) {
            Err(TestkitError::PixelMismatch {
                mismatched_pixels,
                max_channel_diff,
                ref actual_path,
                ref diff_path,
                ..
            }) => {
                assert_eq!(mismatched_pixels, 4, "all 4 pixels were nudged");
                assert_eq!(max_channel_diff, 50);
                assert!(actual_path.exists());
                assert!(diff_path.exists());
            }
            other => unreachable!("expected PixelMismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_dimension_mismatch_is_reported_without_comparing_pixels() {
        let (_dir, path) = golden_path();
        bless(&path, &sample());

        let wrong_size = match Image::new(1, 1, vec![0, 0, 0, 255]) {
            Ok(image) => image,
            Err(err) => unreachable!("{err}"),
        };
        match compare(&path, &wrong_size, 0, false) {
            Err(TestkitError::DimensionMismatch {
                golden_width: 2,
                golden_height: 2,
                actual_width: 1,
                actual_height: 1,
                ..
            }) => {}
            other => unreachable!("expected DimensionMismatch, got {other:?}"),
        }
    }

    /// A golden isn't necessarily a file this crate's own bless mode
    /// wrote -- someone could place a PNG there by hand from another
    /// tool. A grayscale PNG (not `Rgba8`) is exactly that case.
    #[test]
    fn a_golden_in_the_wrong_png_format_is_a_distinct_error_not_a_crash() {
        let (_dir, path) = golden_path();
        let file = match std::fs::File::create(&path) {
            Ok(file) => file,
            Err(err) => unreachable!("{err}"),
        };
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 2, 2);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = match encoder.write_header() {
            Ok(writer) => writer,
            Err(err) => unreachable!("{err}"),
        };
        if let Err(err) = writer.write_image_data(&[0, 0, 0, 0]) {
            unreachable!("{err}");
        }
        drop(writer);

        match compare(&path, &sample(), 0, false) {
            Err(TestkitError::GoldenWrongFormat {
                color_type: png::ColorType::Grayscale,
                ..
            }) => {}
            other => unreachable!("expected GoldenWrongFormat, got {other:?}"),
        }
    }
}
