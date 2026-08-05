//! TIFF import/export — this crate's third real file format (M1.9), via
//! the `tiff` crate (`image-tiff` upstream) — the same image-rs
//! organisation `png` already comes from, and PRD §8.2's own
//! pre-decided "`image`, `zune-image` | Mature, pure Rust" pair covers
//! TIFF too (its own table row lists PNG/JPG/TIFF/WebP/GIF/BMP
//! together). One crate here covers both decode and encode, unlike
//! JPEG's two-crate split.
//!
//! **Scope, stated honestly** — TIFF is a far more permissive container
//! than PNG or JPEG (arbitrary bit depth, photometric interpretation,
//! compression, and multiple pages/IFDs per file), so this is
//! deliberately a first slice, not full coverage:
//! - Only the **first image (IFD)** in a TIFF file is read — real
//!   multi-page TIFF support is separate, still-open follow-on work.
//! - Only `Gray`/`GrayA`/`RGB`/`RGBA` photometric layouts are decoded,
//!   normalized to this crate's own canonical RGBA via the same
//!   `crate::channels` helpers `png` uses. `Palette`/`CMYK`/`CMYKA`
//!   sources are a real, checked error, not a silent misread — CMYK in
//!   particular would need a real ICC-aware conversion
//!   (`aurora_color::Transform`) to convert correctly, which is real,
//!   separate follow-on work, not something to fake with an
//!   uncalibrated formula.
//! - Only 8-bit and 16-bit unsigned-integer samples are decoded (the
//!   same two depths `png` already supports) — 32-bit float TIFFs (a
//!   real, if less common, HDR case) are a checked error, not silently
//!   misinterpreted as integer samples.
//! - Encode is always 8-bit RGBA, uncompressed — matching invariant
//!   §7.3.1b's own 8-bit export boundary via [`dither_quantize`], the
//!   same as `png`/`jpeg`. Compressed TIFF export (LZW/deflate, both of
//!   which the `tiff` crate itself already supports for *decode*) is
//!   real, separate follow-on work.
//! - Colour space is always tagged sRGB — the same honest gap `png`/
//!   `jpeg` already document; TIFF's own embedded-ICC-profile tag isn't
//!   read (or written) yet.

use aurora_color::{IccProfile, dither_quantize, promote_u8, promote_u16};
use half::f16;
use std::io::Cursor;
use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::encoder::TiffEncoder;
use tiff::encoder::colortype::RGBA8;

use crate::channels::{gray_alpha_to_rgba, gray_to_rgba, rgb_to_rgba};
use crate::error::IoError;
use crate::image::Image;

/// Decodes `bytes` (a whole TIFF file's own contents) into a real
/// [`Image`] — only the first image (IFD) in the file, see this
/// module's own doc comment.
///
/// # Errors
///
/// Returns [`IoError::TiffDecode`] if `bytes` isn't a valid TIFF,
/// [`IoError::UnsupportedTiffColorType`] for a photometric layout this
/// crate doesn't handle yet, or [`IoError::UnsupportedTiffSampleFormat`]
/// for a sample type other than 8-/16-bit unsigned integers.
pub fn decode(bytes: &[u8]) -> Result<Image, IoError> {
    let mut decoder = Decoder::new(Cursor::new(bytes))?;
    let (width, height) = decoder.dimensions()?;
    let color_type = decoder.colortype()?;
    let result = decoder.read_image()?;

    let channels = match color_type {
        ColorType::RGBA(_) => 4,
        ColorType::RGB(_) => 3,
        ColorType::GrayA(_) => 2,
        ColorType::Gray(_) => 1,
        other => return Err(IoError::UnsupportedTiffColorType(other)),
    };

    let promoted: Vec<f16> = match result {
        DecodingResult::U8(samples) => samples
            .into_iter()
            .map(|sample| f16::from_f32(promote_u8(sample)))
            .collect(),
        DecodingResult::U16(samples) => samples
            .into_iter()
            .map(|sample| f16::from_f32(promote_u16(sample)))
            .collect(),
        DecodingResult::U32(_) => {
            return Err(IoError::UnsupportedTiffSampleFormat(
                "32-bit unsigned integer",
            ));
        }
        DecodingResult::U64(_) => {
            return Err(IoError::UnsupportedTiffSampleFormat(
                "64-bit unsigned integer",
            ));
        }
        DecodingResult::F16(_) => {
            return Err(IoError::UnsupportedTiffSampleFormat("16-bit float"));
        }
        DecodingResult::F32(_) => {
            return Err(IoError::UnsupportedTiffSampleFormat("32-bit float"));
        }
        DecodingResult::F64(_) => {
            return Err(IoError::UnsupportedTiffSampleFormat("64-bit float"));
        }
        DecodingResult::I8(_) => {
            return Err(IoError::UnsupportedTiffSampleFormat("8-bit signed integer"));
        }
        DecodingResult::I16(_) => {
            return Err(IoError::UnsupportedTiffSampleFormat(
                "16-bit signed integer",
            ));
        }
        DecodingResult::I32(_) => {
            return Err(IoError::UnsupportedTiffSampleFormat(
                "32-bit signed integer",
            ));
        }
        DecodingResult::I64(_) => {
            return Err(IoError::UnsupportedTiffSampleFormat(
                "64-bit signed integer",
            ));
        }
    };

    let samples = match channels {
        4 => promoted,
        3 => rgb_to_rgba(&promoted),
        2 => gray_alpha_to_rgba(&promoted),
        1 => gray_to_rgba(&promoted),
        _ => unreachable!("channels is always one of the four match arms above"),
    };

    Image::new(width, height, IccProfile::srgb(), samples)
}

/// Encodes `image` as an uncompressed, 8-bit RGBA TIFF — the export
/// half of invariant §7.3.1b's 8-bit boundary, using [`dither_quantize`]
/// rather than plain rounding so a smooth gradient doesn't band on the
/// way out.
///
/// # Errors
///
/// Returns [`IoError::TiffEncode`] if the encoder itself fails.
pub fn encode(image: &Image) -> Result<Vec<u8>, IoError> {
    let width = image.width();
    let mut rgba8 = Vec::with_capacity(image.samples().len());
    for (index, &sample) in image.samples().iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let pixel_index = (index / 4) as u32;
        let x = pixel_index % width.max(1);
        let y = pixel_index / width.max(1);
        rgba8.push(dither_quantize(sample.to_f32(), x, y));
    }

    let mut bytes = Cursor::new(Vec::new());
    let mut encoder = TiffEncoder::new(&mut bytes).map_err(IoError::TiffEncode)?;
    encoder
        .write_image::<RGBA8>(width, image.height(), &rgba8)
        .map_err(IoError::TiffEncode)?;
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};
    use crate::Image;
    use aurora_color::{IccProfile, promote_u8, quantize_u8};
    use half::f16;

    #[test]
    fn encode_then_decode_round_trips_within_one_quantization_step() {
        // Uncompressed both ends -- unlike JPEG, TIFF itself is
        // lossless here, but `encode`'s own `dither_quantize` (not
        // plain rounding) deliberately perturbs a value by up to one
        // quantization step at some pixel positions to break up
        // banding -- the same "within one step, not bit-exact" bound
        // `png`'s own round-trip test already uses, for the same
        // reason.
        let width = 4;
        let height = 4;
        let samples: Vec<f16> = (0..width * height * 4)
            .map(|i| f16::from_f32(promote_u8((i * 17 % 256) as u8)))
            .collect();
        let image = match Image::new(width, height, IccProfile::srgb(), samples.clone()) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };

        let bytes = match encode(&image) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let decoded = match decode(&bytes) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };

        assert_eq!(decoded.width(), width);
        assert_eq!(decoded.height(), height);
        for (&original, &round_tripped) in samples.iter().zip(decoded.samples()) {
            let (a, b) = (
                quantize_u8(original.to_f32()),
                quantize_u8(round_tripped.to_f32()),
            );
            assert!(
                a.abs_diff(b) <= 1,
                "expected values within one quantization step, got {a} vs {b}"
            );
        }
    }

    /// Independent-reader-style cross-check, the same discipline `png`'s
    /// own tests already use: builds a real TIFF via the `tiff` crate's
    /// own encoder directly (not this module's `encode`), covering a
    /// photometric layout `encode` itself never produces, so `decode`'s
    /// own channel-expansion path is exercised against real, independent
    /// bytes.
    #[test]
    fn decode_expands_a_real_grayscale_source_to_rgba() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut tiff_encoder = match tiff::encoder::TiffEncoder::new(&mut bytes) {
                Ok(encoder) => encoder,
                Err(err) => unreachable!("{err:?}"),
            };
            // A single opaque mid-gray pixel.
            if let Err(err) =
                tiff_encoder.write_image::<tiff::encoder::colortype::Gray8>(1, 1, &[128])
            {
                unreachable!("{err:?}");
            }
        }

        let image = match decode(bytes.get_ref()) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(image.samples().len(), 4, "must expand to 4 RGBA channels");
        let Some(&red) = image.samples().first() else {
            unreachable!("just asserted len() == 4");
        };
        let Some(&green) = image.samples().get(1) else {
            unreachable!("just asserted len() == 4");
        };
        let Some(&blue) = image.samples().get(2) else {
            unreachable!("just asserted len() == 4");
        };
        let Some(&alpha) = image.samples().get(3) else {
            unreachable!("just asserted len() == 4");
        };
        assert_eq!(red, green, "grayscale must expand equally into R, G, and B");
        assert_eq!(green, blue);
        // Exact-literal comparison, not accumulated computation noise --
        // same reasoning `aurora-color`'s own tests already document.
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(alpha.to_f32(), 1.0);
        }
    }

    #[test]
    fn decode_rejects_a_cmyk_source() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut tiff_encoder = match tiff::encoder::TiffEncoder::new(&mut bytes) {
                Ok(encoder) => encoder,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Err(err) =
                tiff_encoder.write_image::<tiff::encoder::colortype::CMYK8>(1, 1, &[0, 0, 0, 255])
            {
                unreachable!("{err:?}");
            }
        }

        match decode(bytes.get_ref()) {
            Err(crate::IoError::UnsupportedTiffColorType(_)) => {}
            other => unreachable!("expected UnsupportedTiffColorType, got {other:?}"),
        }
    }

    #[test]
    fn decode_rejects_garbage() {
        match decode(b"not a tiff") {
            Err(_) => {}
            Ok(_) => unreachable!("garbage bytes must not decode as a valid TIFF"),
        }
    }
}
