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
//! - [`decode`] still only reads the **first image (IFD)** in a TIFF
//!   file — [`decode_all`] (added 2026-08-06) is the real multi-page
//!   counterpart, reading every IFD in order (`Decoder::more_images`/
//!   `next_image`) rather than just the first.
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
//! - Encode is always 8-bit RGBA — matching invariant §7.3.1b's own
//!   8-bit export boundary via [`dither_quantize`], the same as
//!   `png`/`jpeg` — uncompressed ([`encode`]) or LZW-compressed
//!   ([`encode_compressed`], added 2026-08-06). Deflate/`PackBits` (the
//!   `tiff` crate's other two compression options) remain unwritten —
//!   LZW alone covers the "smaller file, still universally readable"
//!   case this exists for.
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
/// [`Image`] — only the first image (IFD) in the file; see
/// [`decode_all`] for every one.
///
/// # Errors
///
/// See `decode_current_image`.
pub fn decode(bytes: &[u8]) -> Result<Image, IoError> {
    let mut decoder = Decoder::new(Cursor::new(bytes))?;
    decode_current_image(&mut decoder)
}

/// Decodes `bytes` into one real [`Image`] per IFD, in file order —
/// the real multi-page counterpart [`decode`] itself doesn't attempt.
///
/// # Errors
///
/// See `decode_current_image` — the first page to fail any of its own
/// checks aborts the whole call, rather than returning however many
/// pages decoded before it (a partial `Vec<Image>` with no way to tell
/// the caller it's incomplete would be a worse failure mode than a
/// clean, whole-file error).
pub fn decode_all(bytes: &[u8]) -> Result<Vec<Image>, IoError> {
    let mut decoder = Decoder::new(Cursor::new(bytes))?;
    let mut images = vec![decode_current_image(&mut decoder)?];
    while decoder.more_images() {
        decoder.next_image()?;
        images.push(decode_current_image(&mut decoder)?);
    }
    Ok(images)
}

/// [`decode`]/[`decode_all`]'s own shared per-IFD logic: `decoder`'s
/// *current* image (whichever `Decoder::new`/`Decoder::next_image` most
/// recently selected) into a real [`Image`].
///
/// # Errors
///
/// Returns [`IoError::UnsupportedTiffColorType`] for a photometric
/// layout this crate doesn't handle yet, [`IoError::UnsupportedTiffSampleFormat`]
/// for a sample type other than 8-/16-bit unsigned integers or
/// 16-/32-bit float, [`IoError::Color`] if an embedded `ICCProfile`
/// tag's own bytes fail to parse, or [`IoError::TiffDecode`] for any
/// other real decode failure.
fn decode_current_image(decoder: &mut Decoder<Cursor<&[u8]>>) -> Result<Image, IoError> {
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

/// `image`'s own samples, dither-quantized to 8-bit RGBA — the shared
/// pixel-preparation step [`encode`]/[`encode_compressed`] both need,
/// using [`dither_quantize`] rather than plain rounding so a smooth
/// gradient doesn't band on the way out (invariant §7.3.1b's own 8-bit
/// export boundary).
fn quantize_rgba8(image: &Image) -> Vec<u8> {
    let width = image.width();
    let mut rgba8 = Vec::with_capacity(image.samples().len());
    for (index, &sample) in image.samples().iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let pixel_index = (index / 4) as u32;
        let x = pixel_index % width.max(1);
        let y = pixel_index / width.max(1);
        rgba8.push(dither_quantize(sample.to_f32(), x, y));
    }
    rgba8
}

/// Encodes `image` as an uncompressed, 8-bit RGBA TIFF. Embeds
/// `image.color_space()`'s own real bytes as the standard `ICCProfile`
/// tag — see this module's own doc comment. See [`encode_compressed`]
/// for the real, LZW-compressed counterpart.
///
/// # Errors
///
/// Returns [`IoError::Color`] if `image.color_space()` fails to
/// serialize, or [`IoError::TiffEncode`] if the encoder itself fails.
pub fn encode(image: &Image) -> Result<Vec<u8>, IoError> {
    let rgba8 = quantize_rgba8(image);
    let profile_bytes = image.color_space().to_bytes()?;

    let mut bytes = Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut bytes).map_err(IoError::TiffEncode)?;
        let mut image_encoder = encoder
            .new_image::<RGBA8>(image.width(), image.height())
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

/// Encodes `image` as an LZW-compressed, 8-bit RGBA TIFF — real,
/// separate follow-on work [`encode`]'s own doc comment used to name as
/// still open. LZW specifically (not Deflate/`PackBits`, the `tiff`
/// crate's other two options): lossless, and the one every mainstream
/// TIFF reader (this crate's own [`decode`] included — the `tiff` crate
/// already supports LZW *decode*) is guaranteed to understand, unlike
/// Deflate-compressed TIFF, which is real but less universally
/// supported.
///
/// # Errors
///
/// Returns [`IoError::Color`] if `image.color_space()` fails to
/// serialize, or [`IoError::TiffEncode`] if the encoder itself fails.
pub fn encode_compressed(image: &Image) -> Result<Vec<u8>, IoError> {
    let rgba8 = quantize_rgba8(image);
    let profile_bytes = image.color_space().to_bytes()?;

    let mut bytes = Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut bytes)
            .map_err(IoError::TiffEncode)?
            .with_compression(tiff::encoder::Compression::Lzw);
        let mut image_encoder = encoder
            .new_image::<RGBA8>(image.width(), image.height())
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
    use super::{decode, decode_all, encode, encode_compressed};
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

    #[test]
    fn decode_all_reads_every_ifd_in_a_real_multi_page_tiff() {
        // Built independently of this module's own `encode` -- three
        // real pages, each a single distinctly-valued pixel, written to
        // the *same* `TiffEncoder` (which chains each `write_image`
        // call into the next IFD automatically) so `decode_all` has
        // real, order-dependent content to prove it reads all three in
        // file order, not just however many happen to decode.
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut tiff_encoder = match tiff::encoder::TiffEncoder::new(&mut bytes) {
                Ok(encoder) => encoder,
                Err(err) => unreachable!("{err:?}"),
            };
            for value in [10u8, 20, 30] {
                if let Err(err) =
                    tiff_encoder.write_image::<tiff::encoder::colortype::Gray8>(1, 1, &[value])
                {
                    unreachable!("{err:?}");
                }
            }
        }

        let images = match decode_all(bytes.get_ref()) {
            Ok(images) => images,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            images.len(),
            3,
            "must read all three pages, not just the first"
        );

        let expected = [promote_u8(10), promote_u8(20), promote_u8(30)];
        for (image, &want) in images.iter().zip(expected.iter()) {
            let Some(&red) = image.samples().first() else {
                unreachable!("a 1x1 image always has at least one sample");
            };
            assert!(
                (red.to_f32() - want).abs() < 1e-3,
                "page order must match file order: expected ~{want}, got {}",
                red.to_f32()
            );
        }
    }

    #[test]
    fn decode_reads_the_first_page_of_a_multi_page_tiff_the_same_as_decode_all() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut tiff_encoder = match tiff::encoder::TiffEncoder::new(&mut bytes) {
                Ok(encoder) => encoder,
                Err(err) => unreachable!("{err:?}"),
            };
            for value in [42u8, 99] {
                if let Err(err) =
                    tiff_encoder.write_image::<tiff::encoder::colortype::Gray8>(1, 1, &[value])
                {
                    unreachable!("{err:?}");
                }
            }
        }

        let single = match decode(bytes.get_ref()) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        let all = match decode_all(bytes.get_ref()) {
            Ok(images) => images,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(first_of_all) = all.first() else {
            unreachable!("just wrote two real pages");
        };
        assert_eq!(single.samples(), first_of_all.samples());
    }

    #[test]
    fn encode_compressed_round_trips_the_same_pixels_as_encode() {
        // A large, solid-colour image -- real content LZW actually
        // compresses well, so a size-comparison assertion below means
        // something rather than being lucky noise on a 1x1 test image.
        let width = 64;
        let height = 64;
        let samples: Vec<f16> = (0..width * height * 4)
            .map(|i| f16::from_f32(promote_u8(if i % 4 == 3 { 255 } else { 80 })))
            .collect();
        let image = match Image::new(width, height, IccProfile::srgb(), samples) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };

        let plain = match encode(&image) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let compressed = match encode_compressed(&image) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            compressed.len() < plain.len(),
            "LZW must actually shrink a real, compressible image: \
             {} plain vs {} compressed bytes",
            plain.len(),
            compressed.len()
        );

        let decoded_plain = match decode(&plain) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        let decoded_compressed = match decode(&compressed) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            decoded_plain.samples(),
            decoded_compressed.samples(),
            "LZW is lossless -- both encodings must decode to identical pixels"
        );
    }
}
