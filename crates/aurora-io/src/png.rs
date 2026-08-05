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
//! - Encode is always 8-bit — PNG's own most common case, and the one
//!   invariant §7.3.1b itself names. 16-bit *export* is real, separate
//!   follow-on work, not implemented here.
//! - Colour space is always tagged sRGB
//!   ([`aurora_color::IccProfile::srgb`]) — PNG's `iCCP`/embedded-
//!   profile chunks are not read (or written) yet. An honest gap for a
//!   real, if uncommon, minority of PNGs that do carry a different
//!   embedded profile, not a silently wrong answer for the common case.

use aurora_color::{IccProfile, dither_quantize, promote_u8, promote_u16};
use half::f16;

use crate::error::IoError;
use crate::image::Image;

/// Decodes `bytes` (a whole PNG file's own contents) into a real
/// [`Image`].
///
/// # Errors
///
/// Returns [`IoError::PngDecode`] if `bytes` isn't a valid PNG, or
/// [`IoError::UnexpectedColorType`] if decoding produced a colour
/// layout that isn't RGBA or `GrayscaleAlpha` (the only two real
/// post-transformation layouts `png`'s own `EXPAND`/`ALPHA` flags ever
/// produce — see this module's own doc comment).
pub fn decode(bytes: &[u8]) -> Result<Image, IoError> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info()?;
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

    Image::new(info.width, info.height, IccProfile::srgb(), samples)
}

/// Encodes `image` as an 8-bit PNG (see this module's own doc comment
/// for why not 16-bit) — the export half of invariant §7.3.1b's 8-bit
/// boundary, using [`dither_quantize`] rather than plain rounding so a
/// smooth gradient doesn't band on the way out.
///
/// # Errors
///
/// Returns [`IoError::PngEncode`] if the encoder itself fails (e.g. an
/// unwritable output buffer).
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

    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width(), image.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&rgba8)?;
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
}
