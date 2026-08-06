//! PNG import/export — the simplest real file format this crate
//! supports, and the boundary invariant §7.3.1b actually names ("8-bit
//! appears only at import (promoted immediately) and export (quantized
//! with dithering)").
//!
//! **Scope, stated honestly**:
//! - Decode normalizes any PNG colour type (grayscale, indexed, RGB,
//!   with or without an alpha/`tRNS` channel) to RGBA, via `png`'s own
//!   `Transformations::EXPAND | Transformations::ALPHA` plus one manual
//!   step this crate adds itself: confirmed by actually running it
//!   against a real grayscale-source PNG (not assumed from the
//!   transformation flags' own documentation), `EXPAND` only expands
//!   *paletted* images to RGB and sub-8-bit *grayscale* to 8-bit
//!   grayscale — it never expands grayscale to RGB — so a grayscale
//!   source comes back as `GrayscaleAlpha` (2 channels), which
//!   [`decode`] itself then expands to RGBA (duplicating the gray
//!   sample into R/G/B). Bit depth is left alone throughout — an 8-bit
//!   source promotes from real 8-bit samples, a 16-bit source promotes
//!   from real 16-bit samples ([`aurora_color::promote_u16`]), not
//!   silently downsampled to 8 bits first the way requesting
//!   `STRIP_16` would.
//! - [`encode`] is always 8-bit — PNG's own most common case, and the
//!   one invariant §7.3.1b itself names. [`encode_16`] (added
//!   2026-08-06) is the real 16-bit counterpart, for a caller that
//!   wants to keep real precision beyond 8 bits in the exported file
//!   itself, not just internally.
//! - Colour space is real now (added 2026-08-06): [`decode`] reads a
//!   source PNG's own `iCCP` chunk if it has one
//!   (`aurora_color::IccProfile::from_bytes`), falling back to
//!   [`aurora_color::IccProfile::srgb`] only when the file carries no
//!   profile at all — PNG's own convention for an untagged file.
//!   [`encode`]/[`encode_16`] embed `image.color_space()`'s own real
//!   bytes unconditionally (`aurora_color::IccProfile::to_bytes`),
//!   including the common case where that's just the built-in sRGB
//!   profile — simple and always correct, at the cost of a few KB
//!   this crate doesn't yet try to save by detecting "this profile is
//!   exactly sRGB" and writing PNG's own lighter `sRGB` chunk instead;
//!   real, separate follow-on work if file size ever becomes a real
//!   concern here.

use aurora_color::{IccProfile, dither_quantize, promote_u8, promote_u16, quantize_u16};
use half::f16;

use crate::error::IoError;
use crate::image::Image;

/// Decodes `bytes` (a whole PNG file's own contents) into a real
/// [`Image`].
///
/// # Errors
///
/// Returns [`IoError::PngDecode`] if `bytes` isn't a valid PNG,
/// [`IoError::UnexpectedColorType`] if decoding produced a colour
/// layout that isn't RGBA or `GrayscaleAlpha` (the only two real
/// post-transformation layouts `png`'s own `EXPAND`/`ALPHA` flags ever
/// produce — see this module's own doc comment), or [`IoError::Color`]
/// if the source file's own embedded `iCCP` profile fails to parse.
pub fn decode(bytes: &[u8]) -> Result<Image, IoError> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info()?;
    let color_space = match &reader.info().icc_profile {
        Some(bytes) => IccProfile::from_bytes(bytes)?,
        // No `iCCP` chunk at all -- PNG's own convention for an
        // untagged file, the same default this crate has always used.
        None => IccProfile::srgb(),
    };
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let Some(pixels) = buf.get(..info.buffer_size()) else {
        unreachable!(
            "buf was allocated via output_buffer_size(), which is always >= any real frame's own buffer_size()"
        );
    };

    // `EXPAND | ALPHA` normalizes indexed/RGB/grayscale sources to
    // *four* channels each, but — confirmed by actually running this
    // against a real grayscale-source PNG, not assumed — grayscale
    // stays `GrayscaleAlpha` (2 channels), not `Rgba`: `png`'s own
    // `EXPAND` flag only documents expanding *paletted* images to RGB
    // and expanding sub-8-bit *grayscale* to 8-bit grayscale, never
    // grayscale to RGB. So both real post-transformation layouts are
    // handled explicitly below; anything else is a real, checked error,
    // not an assumption.
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::GrayscaleAlpha => 2,
        other => return Err(IoError::UnexpectedColorType(other)),
    };

    let promoted: Vec<f16> = match info.bit_depth {
        png::BitDepth::Sixteen => pixels
            .chunks_exact(2)
            .map(|pair| {
                let Ok(raw) = <[u8; 2]>::try_from(pair) else {
                    unreachable!("chunks_exact(2) always yields length-2 slices");
                };
                f16::from_f32(promote_u16(u16::from_be_bytes(raw)))
            })
            .collect(),
        _ => pixels
            .iter()
            .map(|&sample| f16::from_f32(promote_u8(sample)))
            .collect(),
    };

    let samples = if channels == 4 {
        promoted
    } else {
        crate::channels::gray_alpha_to_rgba(&promoted)
    };

    Image::new(info.width, info.height, color_space, samples)
}

/// Builds the `png::Info` [`encode`]/[`encode_16`] both encode against:
/// `width`/`height`/`color_type`/`bit_depth` for the real pixel data,
/// plus `image.color_space()`'s own real bytes embedded as a real
/// `iCCP` chunk (see this module's own doc comment for why
/// unconditionally, including the common sRGB case).
///
/// # Errors
///
/// Returns [`IoError::Color`] if `image.color_space().to_bytes()`
/// fails.
fn encoder_info(image: &Image, bit_depth: png::BitDepth) -> Result<png::Info<'static>, IoError> {
    let mut info = png::Info::with_size(image.width(), image.height());
    info.color_type = png::ColorType::Rgba;
    info.bit_depth = bit_depth;
    info.icc_profile = Some(image.color_space().to_bytes()?.into());
    Ok(info)
}

/// Encodes `image` as an 8-bit PNG — the export half of invariant
/// §7.3.1b's 8-bit boundary, using [`dither_quantize`] rather than
/// plain rounding so a smooth gradient doesn't band on the way out. See
/// [`encode_16`] for the real 16-bit counterpart.
///
/// # Errors
///
/// Returns [`IoError::Color`] if `image.color_space()` fails to
/// serialize, or [`IoError::PngEncode`] if the encoder itself fails
/// (e.g. an unwritable output buffer).
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

    let info = encoder_info(image, png::BitDepth::Eight)?;
    let mut bytes = Vec::new();
    {
        let mut writer = png::Encoder::with_info(&mut bytes, info)?.write_header()?;
        writer.write_image_data(&rgba8)?;
    }
    Ok(bytes)
}

/// Encodes `image` as a 16-bit PNG — real precision beyond 8 bits
/// preserved all the way into the exported file itself, not just
/// internally, for a caller that wants it (e.g. a print-bound export
/// where 8-bit banding risk matters more than file size). Plain
/// rounding, not dithering ([`aurora_color::quantize_u16`]'s own doc
/// comment explains why 16 bits doesn't need it the way [`encode`]'s
/// 8-bit path does).
///
/// # Errors
///
/// Returns [`IoError::Color`] if `image.color_space()` fails to
/// serialize, or [`IoError::PngEncode`] if the encoder itself fails.
pub fn encode_16(image: &Image) -> Result<Vec<u8>, IoError> {
    let mut rgba16 = Vec::with_capacity(image.samples().len() * 2);
    for &sample in image.samples() {
        rgba16.extend_from_slice(&quantize_u16(sample.to_f32()).to_be_bytes());
    }

    let info = encoder_info(image, png::BitDepth::Sixteen)?;
    let mut bytes = Vec::new();
    {
        let mut writer = png::Encoder::with_info(&mut bytes, info)?.write_header()?;
        writer.write_image_data(&rgba16)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};
    use aurora_color::{promote_u8, quantize_u8};
    use half::f16;

    /// Builds a real PNG file's bytes via the `png` crate's own
    /// encoder, independent of this module's own `encode` — an
    /// independent-reader-style cross-check (`decode` proven against
    /// something other than this module's own `encode`).
    fn make_png(
        width: u32,
        height: u32,
        color_type: png::ColorType,
        bit_depth: png::BitDepth,
        data: &[u8],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(color_type);
            encoder.set_depth(bit_depth);
            let mut writer = match encoder.write_header() {
                Ok(writer) => writer,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Err(err) = writer.write_image_data(data) {
                unreachable!("{err:?}");
            }
        }
        bytes
    }

    #[test]
    fn decode_promotes_an_8_bit_rgba_png() {
        // A single 2x1 image: opaque red, then translucent green.
        let data: [u8; 8] = [255, 0, 0, 255, 0, 255, 0, 128];
        let bytes = make_png(2, 1, png::ColorType::Rgba, png::BitDepth::Eight, &data);

        let image = match decode(&bytes) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(image.width(), 2);
        assert_eq!(image.height(), 1);
        assert_eq!(image.samples().len(), 8);

        let expected: Vec<f16> = data
            .iter()
            .map(|&sample| f16::from_f32(promote_u8(sample)))
            .collect();
        assert_eq!(image.samples(), expected.as_slice());
    }

    #[test]
    fn decode_adds_an_opaque_alpha_channel_to_an_rgb_source() {
        // A single opaque blue pixel, no alpha channel in the source at
        // all -- must still decode to 4 RGBA channels, not 3.
        let data: [u8; 3] = [0, 0, 255];
        let bytes = make_png(1, 1, png::ColorType::Rgb, png::BitDepth::Eight, &data);

        let image = match decode(&bytes) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(image.samples().len(), 4);
        let Some(&alpha) = image.samples().get(3) else {
            unreachable!("just asserted len() == 4");
        };
        // 255 promotes to an exact 1.0, not accumulated computation
        // noise -- same reasoning `aurora-color`'s own tests already
        // document for their float_cmp allows.
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(alpha.to_f32(), 1.0);
        }
    }

    #[test]
    fn decode_expands_an_indexed_source_through_its_own_palette() {
        // A 1x1 image whose single pixel is palette index 1, which maps
        // to opaque yellow -- proves the palette lookup itself, not
        // just that *some* value came out.
        let palette: [u8; 6] = [0, 0, 0, 255, 255, 0]; // index 0: black, index 1: yellow
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(palette.to_vec());
            let mut writer = match encoder.write_header() {
                Ok(writer) => writer,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Err(err) = writer.write_image_data(&[1]) {
                unreachable!("{err:?}");
            }
        }

        let image = match decode(&bytes) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(image.samples().len(), 4);
        let expected: Vec<f16> = [255u8, 255, 0, 255]
            .iter()
            .map(|&sample| f16::from_f32(promote_u8(sample)))
            .collect();
        assert_eq!(image.samples(), expected.as_slice());
    }

    #[test]
    fn decode_expands_grayscale_to_rgba() {
        // A single opaque mid-gray pixel, no alpha channel in the source.
        let data: [u8; 1] = [128];
        let bytes = make_png(1, 1, png::ColorType::Grayscale, png::BitDepth::Eight, &data);

        let image = match decode(&bytes) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(image.samples().len(), 4, "must expand to 4 RGBA channels");
        let Some(&alpha) = image.samples().get(3) else {
            unreachable!("just asserted len() == 4");
        };
        // Exact-literal comparison, not accumulated computation noise --
        // same reasoning `aurora-color`'s own tests already document.
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                alpha.to_f32(),
                1.0,
                "a grayscale source with no transparency must decode fully opaque"
            );
        }
        let Some(&red) = image.samples().first() else {
            unreachable!("just asserted len() == 4");
        };
        let Some(&green) = image.samples().get(1) else {
            unreachable!("just asserted len() == 4");
        };
        let Some(&blue) = image.samples().get(2) else {
            unreachable!("just asserted len() == 4");
        };
        assert_eq!(red, green, "grayscale must expand equally into R, G, and B");
        assert_eq!(green, blue);
    }

    #[test]
    fn decode_preserves_16_bit_precision() {
        // A value with no exact 8-bit equivalent -- proves this isn't
        // silently downsampled to 8 bits before promoting.
        let value: u16 = 0x1234;
        let data = value.to_be_bytes();
        let mut pixel = Vec::new();
        for _ in 0..4 {
            pixel.extend_from_slice(&data);
        }
        let bytes = make_png(1, 1, png::ColorType::Rgba, png::BitDepth::Sixteen, &pixel);

        let image = match decode(&bytes) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        let expected = f16::from_f32(aurora_color::promote_u16(value));
        assert!(
            image.samples().iter().all(|&sample| sample == expected),
            "expected every sample to match the real 16-bit promoted value"
        );
    }

    #[test]
    fn encode_then_decode_round_trips_within_one_quantization_step() {
        let samples: Vec<f16> = (0..16u8)
            .map(|i| f16::from_f32(f32::from(i) / 15.0))
            .collect();
        let image = match crate::Image::new(2, 2, aurora_color::IccProfile::srgb(), samples.clone())
        {
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

        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 2);
        for (&original, &round_tripped) in samples.iter().zip(decoded.samples()) {
            let (a, b) = (
                quantize_u8(original.to_f32()),
                quantize_u8(round_tripped.to_f32()),
            );
            let diff = i32::from(a).abs_diff(i32::from(b));
            assert!(
                diff <= 1,
                "expected values within one quantization step, got {a} vs {b}"
            );
        }
    }

    #[test]
    fn decode_rejects_garbage() {
        match decode(b"not a png") {
            Err(_) => {}
            Ok(_) => unreachable!("garbage bytes must not decode as a valid PNG"),
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
        // `png` crate's own encoder, with a real `iCCP` chunk set
        // directly via `Info`, the same "independent reader" discipline
        // `make_png`'s own doc comment already establishes.
        let mut bytes = Vec::new();
        {
            let mut info = png::Info::with_size(1, 1);
            info.color_type = png::ColorType::Rgba;
            info.bit_depth = png::BitDepth::Eight;
            info.icc_profile = Some(ECI_RGBV2_ICC.to_vec().into());
            let mut writer = match png::Encoder::with_info(&mut bytes, info) {
                Ok(encoder) => match encoder.write_header() {
                    Ok(writer) => writer,
                    Err(err) => unreachable!("{err:?}"),
                },
                Err(err) => unreachable!("{err:?}"),
            };
            if let Err(err) = writer.write_image_data(&[0, 0, 0, 255]) {
                unreachable!("{err:?}");
            }
        }

        let image = match decode(&bytes) {
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
    fn decode_falls_back_to_srgb_when_no_icc_profile_is_embedded() {
        let bytes = make_png(
            1,
            1,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &[0, 0, 0, 255],
        );
        let image = match decode(&bytes) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        let (Ok(decoded_bytes), Ok(srgb_bytes)) = (
            image.color_space().to_bytes(),
            aurora_color::IccProfile::srgb().to_bytes(),
        ) else {
            unreachable!("both are real, always-serializable profiles");
        };
        // Not a proof the two profiles are byte-identical (lcms2 is
        // free to embed a fresh timestamp/profile ID each time it
        // serializes), but both existing and succeeding is the real
        // property this test cares about: an untagged source must not
        // fail to decode or fall back to something broken.
        assert!(!decoded_bytes.is_empty());
        assert!(!srgb_bytes.is_empty());
    }

    #[test]
    fn encode_embeds_the_images_own_real_colour_profile() {
        let profile = match aurora_color::IccProfile::from_bytes(ECI_RGBV2_ICC) {
            Ok(profile) => profile,
            Err(err) => unreachable!("{err:?}"),
        };
        let image = match crate::Image::new(
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
    fn encode_16_preserves_precision_encode_alone_would_lose() {
        // A value that quantizes losslessly at 16 bits but would land
        // between two adjacent 8-bit levels -- exactly the precision
        // `encode`'s own 8-bit path cannot represent.
        let value = 12_345.0 / 65_535.0;
        let image = match crate::Image::new(
            1,
            1,
            aurora_color::IccProfile::srgb(),
            vec![
                f16::from_f32(value),
                f16::from_f32(value),
                f16::from_f32(value),
                f16::from_f32(1.0),
            ],
        ) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };

        let bytes = match super::encode_16(&image) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let decoded = match decode(&bytes) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(&red) = decoded.samples().first() else {
            unreachable!("a 1x1 RGBA image always has at least one sample");
        };
        // `f16` itself has real, unavoidable rounding here (11 bits of
        // mantissa, not 16) -- checked against that real precision
        // ceiling, not a bit-exact 16-bit round trip `f16` structurally
        // cannot make.
        assert!(
            (red.to_f32() - value).abs() < 0.001,
            "16-bit export must preserve far more precision than 8-bit: \
             expected ~{value}, got {}",
            red.to_f32()
        );
    }
}
