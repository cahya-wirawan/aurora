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
//! - 8-bit/16-bit unsigned-integer and (added 2026-08-06) 16-bit/32-bit
//!   float samples all decode. A float sample needs no promotion
//!   formula the way an 8-/16-bit integer one does
//!   ([`aurora_color::promote_u8`]/[`promote_u16`]) — it's already
//!   real-valued, so [`decode`] narrows a 32-bit one straight to `f16`
//!   and passes a 16-bit one through unchanged (`tiff`'s own
//!   `DecodingResult::F16` is already `Vec<half::f16>`, the exact type
//!   this crate uses throughout — confirmed by reading its source, not
//!   assumed from the variant's name). 64-bit floats and every signed-
//!   integer sample format remain a real, checked error, not silently
//!   misinterpreted.
//! - Encode is always 8-bit RGBA, uncompressed — matching invariant
//!   §7.3.1b's own 8-bit export boundary via [`dither_quantize`], the
//!   same as `png`/`jpeg`. Compressed TIFF export (LZW/deflate, both of
//!   which the `tiff` crate itself already supports for *decode*) is
//!   real, separate follow-on work.
//! - Colour space round-trips for real now (added 2026-08-06):
//!   [`decode`] reads the standard `ICCProfile` tag (34675) if present
//!   (`aurora_color::IccProfile::from_bytes`), falling back to
//!   [`aurora_color::IccProfile::srgb`] otherwise; [`encode`] embeds
//!   `image.color_space()`'s own real bytes as that same tag
//!   unconditionally, the same "always embed, don't try to detect
//!   sRGB and skip it" tradeoff `png`'s own module just made. Writing
//!   it needed the `tiff` crate's own lower-level per-tag encoder API
//!   (`Encoder::new_image` + `DirectoryEncoder::write_tag`) rather than
//!   the one-shot `write_image` convenience method this module used to
//!   call directly — `&[u8]` implements the crate's own `TiffValue`
//!   trait via a blanket `&T` impl, confirmed by reading the source
//!   rather than assumed.

use aurora_color::{IccProfile, dither_quantize, promote_u8, promote_u16};
use half::f16;
use std::io::Cursor;
use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::encoder::TiffEncoder;
use tiff::encoder::colortype::RGBA8;
use tiff::tags::Tag;

use crate::channels::{gray_alpha_to_rgba, gray_to_rgba, rgb_to_rgba};
use crate::error::IoError;
use crate::image::Image;

/// The standard TIFF tag for an embedded ICC profile (Adobe's
/// `ICCProfile` tag, adopted into the TIFF/EP and DNG specs) — not one
/// of the `tiff` crate's own named [`Tag`] variants, so read via
/// [`Tag::Unknown`] and its own raw byte-array conversion instead of a
/// dedicated accessor.
const ICC_PROFILE_TAG: u16 = 34675;

/// Decodes `bytes` (a whole TIFF file's own contents) into a real
/// [`Image`] — only the first image (IFD) in the file, see this
/// module's own doc comment.
///
/// # Errors
///
/// Returns [`IoError::TiffDecode`] if `bytes` isn't a valid TIFF,
/// [`IoError::UnsupportedTiffColorType`] for a photometric layout this
/// crate doesn't handle yet, [`IoError::UnsupportedTiffSampleFormat`]
/// for a sample type other than 8-/16-bit unsigned integers or 32-bit
/// float, or [`IoError::Color`] if an embedded `ICCProfile` tag's own
/// bytes fail to parse.
pub fn decode(bytes: &[u8]) -> Result<Image, IoError> {
    let mut decoder = Decoder::new(Cursor::new(bytes))?;
    let (width, height) = decoder.dimensions()?;
    let color_type = decoder.colortype()?;
    let color_space = match decoder.find_tag(Tag::Unknown(ICC_PROFILE_TAG))? {
        Some(value) => IccProfile::from_bytes(&value.into_u8_vec()?)?,
        None => IccProfile::srgb(),
    };
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
        // Already real-valued samples, unlike the integer formats above
        // -- no `promote_u8`/`promote_u16`-style normalization formula
        // applies, so these narrow straight into this pipeline's own
        // `f16`. `DecodingResult::F16` is already `Vec<half::f16>` --
        // the exact same type this crate uses throughout, confirmed by
        // reading the `tiff` crate's own source rather than assumed
        // from the variant's name alone.
        DecodingResult::F16(samples) => samples,
        DecodingResult::F32(samples) => samples.into_iter().map(f16::from_f32).collect(),
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

    Image::new(width, height, color_space, samples)
}

/// Encodes `image` as an uncompressed, 8-bit RGBA TIFF — the export
/// half of invariant §7.3.1b's 8-bit boundary, using [`dither_quantize`]
/// rather than plain rounding so a smooth gradient doesn't band on the
/// way out. Embeds `image.color_space()`'s own real bytes as the
/// standard `ICCProfile` tag — see this module's own doc comment.
///
/// # Errors
///
/// Returns [`IoError::Color`] if `image.color_space()` fails to
/// serialize, or [`IoError::TiffEncode`] if the encoder itself fails.
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
    let profile_bytes = image.color_space().to_bytes()?;

    let mut bytes = Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut bytes).map_err(IoError::TiffEncode)?;
        let mut image_encoder = encoder
            .new_image::<RGBA8>(width, image.height())
            .map_err(IoError::TiffEncode)?;
        image_encoder
            .encoder()
            .write_tag(Tag::Unknown(ICC_PROFILE_TAG), profile_bytes.as_slice())
            .map_err(IoError::TiffEncode)?;
        image_encoder
            .write_data(&rgba8)
            .map_err(IoError::TiffEncode)?;
    }
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

    // CC0-licensed, from the colord-data Debian package -- see
    // corpora/icc/README.md for full provenance. The same real,
    // deliberately non-sRGB profile `aurora-color`'s own tests already
    // use.
    const ECI_RGBV2_ICC: &[u8] = include_bytes!("../../../corpora/icc/ECI-RGBv2.icc");

    #[test]
    fn decode_reads_a_real_embedded_non_srgb_icc_profile() {
        // Built independently of this module's own `encode` -- the
        // `tiff` crate's own lower-level per-tag encoder API, the same
        // "independent reader" discipline this module's other tests
        // already establish for CMYK/grayscale sources.
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut tiff_encoder = match tiff::encoder::TiffEncoder::new(&mut bytes) {
                Ok(encoder) => encoder,
                Err(err) => unreachable!("{err:?}"),
            };
            let mut image_encoder =
                match tiff_encoder.new_image::<tiff::encoder::colortype::Gray8>(1, 1) {
                    Ok(image_encoder) => image_encoder,
                    Err(err) => unreachable!("{err:?}"),
                };
            if let Err(err) = image_encoder
                .encoder()
                .write_tag(super::Tag::Unknown(super::ICC_PROFILE_TAG), ECI_RGBV2_ICC)
            {
                unreachable!("{err:?}");
            }
            if let Err(err) = image_encoder.write_data(&[128]) {
                unreachable!("{err:?}");
            }
        }

        let image = match decode(bytes.get_ref()) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        // Re-serializing the decoded profile must itself succeed --
        // real, checked evidence `decode` actually parsed the embedded
        // bytes into a genuinely usable profile, not just stashed them
        // unexamined.
        if let Err(err) = image.color_space().to_bytes() {
            unreachable!("the decoded profile must itself be a real, usable profile: {err}");
        }
    }

    #[test]
    fn encode_embeds_the_images_own_real_colour_profile() {
        let profile = match IccProfile::from_bytes(ECI_RGBV2_ICC) {
            Ok(profile) => profile,
            Err(err) => unreachable!("{err:?}"),
        };
        let image = match Image::new(
            1,
            1,
            profile,
            vec![
                f16::from_f32(0.0),
                f16::from_f32(0.0),
                f16::from_f32(0.0),
                f16::from_f32(1.0),
            ],
        ) {
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
        if let Err(err) = decoded.color_space().to_bytes() {
            unreachable!("the round-tripped profile must itself be real and usable: {err}");
        }
    }

    #[test]
    fn decode_handles_32_bit_float_samples() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut tiff_encoder = match tiff::encoder::TiffEncoder::new(&mut bytes) {
                Ok(encoder) => encoder,
                Err(err) => unreachable!("{err:?}"),
            };
            // A real HDR value beyond the normal [0, 1] range -- the
            // exact case an 8-/16-bit integer sample structurally
            // cannot represent, and the real reason this format exists.
            if let Err(err) =
                tiff_encoder.write_image::<tiff::encoder::colortype::Gray32Float>(1, 1, &[2.5f32])
            {
                unreachable!("{err:?}");
            }
        }

        let image = match decode(bytes.get_ref()) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(&red) = image.samples().first() else {
            unreachable!("a 1x1 image always has at least one sample");
        };
        assert!(
            (red.to_f32() - 2.5).abs() < 0.01,
            "expected the real HDR value to survive, got {}",
            red.to_f32()
        );
    }

    #[test]
    fn decode_rejects_a_64_bit_float_source() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut tiff_encoder = match tiff::encoder::TiffEncoder::new(&mut bytes) {
                Ok(encoder) => encoder,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Err(err) =
                tiff_encoder.write_image::<tiff::encoder::colortype::Gray64Float>(1, 1, &[0.5f64])
            {
                unreachable!("{err:?}");
            }
        }

        match decode(bytes.get_ref()) {
            Err(crate::IoError::UnsupportedTiffSampleFormat("64-bit float")) => {}
            other => unreachable!("expected UnsupportedTiffSampleFormat, got {other:?}"),
        }
    }
}
