//! JPEG import/export — this crate's second real file format (M1.9),
//! via the two focused, pure-Rust crates PRD §8.2's own "`image`,
//! `zune-image` | Mature, pure Rust" pairing points at: `zune-jpeg`
//! (decode) and `jpeg-encoder` (encode) — `zune-jpeg` is decode-only,
//! so encoding needs a separate crate, matching this module's own
//! "focused, single-purpose codec crate, not the aggregate `image`
//! crate" shape (the same one `png` already established).
//!
//! **Scope, stated honestly**:
//! - JPEG has no native alpha channel. Decode always produces a fully
//!   opaque image (alpha = 1.0 everywhere); encode discards whatever
//!   alpha [`Image`] carries (`jpeg_encoder::ColorType::Rgba`'s own
//!   documented behaviour: "the alpha channel will be ignored during
//!   encoding").
//! - JPEG samples are 8-bit only — there is no mainstream 16-bit JPEG
//!   variant the way PNG has one, so both decode and encode are 8-bit
//!   throughout with no further nuance needed at invariant §7.3.1b's
//!   own boundary.
//! - JPEG's own SOF marker stores dimensions as 16-bit fields — a real,
//!   permanent format limit (65,535×65,535 px), not a library
//!   shortcoming. [`encode`] checks this explicitly rather than
//!   silently truncating a too-large [`Image`].
//! - Colour space is always tagged sRGB, the same honest gap `png`'s
//!   own module already documents: real JPEGs can carry ICC profiles
//!   or an Adobe APP14 marker naming CMYK/YCCK; neither is read here.
//! - Quality is a fixed constant for this first slice — a user-facing
//!   quality control is real, separate follow-on work.

use aurora_color::{IccProfile, dither_quantize, promote_u8};
use half::f16;
use jpeg_encoder::{ColorType, Encoder};
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::error::IoError;
use crate::image::Image;

/// A fixed, "visually near-lossless" default quality (0–100) — no
/// user-facing quality control exists yet (real, separate follow-on
/// work).
const DEFAULT_QUALITY: u8 = 90;

/// Decodes `bytes` (a whole JPEG file's own contents) into a real
/// [`Image`].
///
/// # Errors
///
/// Returns [`IoError::JpegDecode`] if `bytes` isn't a valid JPEG, or
/// [`IoError::UnexpectedJpegColorSpace`] if decoding produced something
/// other than RGBA (see this module's own doc comment for why that's a
/// real, checked possibility, not just theoretical).
pub fn decode(bytes: &[u8]) -> Result<Image, IoError> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let pixels = decoder.decode()?;

    let Some(actual) = decoder.output_colorspace() else {
        unreachable!("decode() succeeding means decode_headers() already ran internally");
    };
    if actual != ColorSpace::RGBA {
        return Err(IoError::UnexpectedJpegColorSpace(actual));
    }

    let Some(info) = decoder.info() else {
        unreachable!("decode() succeeding means decode_headers() already ran internally");
    };

    let samples: Vec<f16> = pixels
        .iter()
        .map(|&sample| f16::from_f32(promote_u8(sample)))
        .collect();

    Image::new(
        u32::from(info.width),
        u32::from(info.height),
        IccProfile::srgb(),
        samples,
    )
}

/// Encodes `image` as a JPEG at `DEFAULT_QUALITY` — the export half
/// of invariant §7.3.1b's 8-bit boundary, using [`dither_quantize`]
/// rather than plain rounding so a smooth gradient doesn't band on the
/// way out. `image`'s own alpha channel is discarded (see this
/// module's own doc comment — JPEG has none to encode it into).
///
/// # Errors
///
/// Returns [`IoError::JpegDimensionsTooLarge`] if `image` is wider or
/// taller than JPEG's own 16-bit dimension fields can represent, or
/// [`IoError::JpegEncode`] if the encoder itself fails.
pub fn encode(image: &Image) -> Result<Vec<u8>, IoError> {
    let (width, height) = (image.width(), image.height());
    let (Ok(width16), Ok(height16)) = (u16::try_from(width), u16::try_from(height)) else {
        return Err(IoError::JpegDimensionsTooLarge { width, height });
    };

    let mut rgba8 = Vec::with_capacity(image.samples().len());
    for (index, &sample) in image.samples().iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let pixel_index = (index / 4) as u32;
        let x = pixel_index % width.max(1);
        let y = pixel_index / width.max(1);
        rgba8.push(dither_quantize(sample.to_f32(), x, y));
    }

    let mut bytes = Vec::new();
    let encoder = Encoder::new(&mut bytes, DEFAULT_QUALITY);
    encoder.encode(&rgba8, width16, height16, ColorType::Rgba)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};
    use crate::Image;
    use aurora_color::{IccProfile, promote_u8, quantize_u8};
    use half::f16;

    #[test]
    fn encode_then_decode_round_trips_within_jpegs_own_lossy_tolerance() {
        // A real gradient, not a flat colour -- flat colours survive
        // JPEG's own lossy compression almost perfectly and wouldn't
        // exercise the DCT/quantization path the way a gradient does.
        let width = 16;
        let height = 16;
        let samples: Vec<f16> = (0..width * height * 4)
            .map(|i| f16::from_f32((i % 256) as f32 / 255.0))
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
        assert_eq!(decoded.samples().len(), samples.len());

        // JPEG is lossy, so this isn't a bit-exact round trip -- but the
        // *colour* channels must stay in the right ballpark, not
        // scramble the data. Alpha is deliberately excluded: JPEG has
        // none, so `decode` always reports it fully opaque regardless
        // of whatever the original alpha sample happened to be --
        // comparing it here would be testing a property this format
        // was never going to preserve, not a real regression.
        let mut max_diff: i32 = 0;
        for (original_pixel, round_tripped_pixel) in samples
            .chunks_exact(4)
            .zip(decoded.samples().chunks_exact(4))
        {
            for channel in 0..3 {
                let (Some(&original), Some(&round_tripped)) = (
                    original_pixel.get(channel),
                    round_tripped_pixel.get(channel),
                ) else {
                    unreachable!("chunks_exact(4) always yields length-4 slices");
                };
                let (a, b) = (
                    quantize_u8(original.to_f32()),
                    quantize_u8(round_tripped.to_f32()),
                );
                max_diff = max_diff.max(i32::from(a.abs_diff(b)));
            }
        }
        assert!(
            max_diff < 40,
            "expected a lossy-but-close round trip, max channel diff was {max_diff}"
        );
    }

    #[test]
    fn decode_produces_a_fully_opaque_image() {
        let width = 4;
        let height = 4;
        let samples: Vec<f16> = (0..width * height * 4)
            .map(|_| f16::from_f32(0.25))
            .collect();
        let image = match Image::new(width, height, IccProfile::srgb(), samples) {
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

        for chunk in decoded.samples().chunks_exact(4) {
            let Some(&alpha) = chunk.get(3) else {
                unreachable!("chunks_exact(4) always yields length-4 slices");
            };
            // Exact-literal comparison, not accumulated computation
            // noise -- decode always sets alpha to the exact promoted
            // value of a full-intensity byte.
            #[allow(clippy::float_cmp)]
            {
                assert_eq!(alpha.to_f32(), f16::from_f32(promote_u8(255)).to_f32());
            }
        }
    }

    #[test]
    fn encode_rejects_dimensions_beyond_jpegs_own_limit() {
        // 70_000 exceeds u16::MAX (65_535) -- a real width JPEG's own
        // SOF marker structurally cannot represent.
        let width = 70_000;
        let height = 1;
        let samples = vec![f16::from_f32(0.0); (width * height * 4) as usize];
        let image = match Image::new(width, height, IccProfile::srgb(), samples) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };

        match encode(&image) {
            Err(crate::IoError::JpegDimensionsTooLarge { width, height }) => {
                assert_eq!(width, 70_000);
                assert_eq!(height, 1);
            }
            other => unreachable!("expected JpegDimensionsTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn decode_rejects_garbage() {
        match decode(b"not a jpeg") {
            Err(_) => {}
            Ok(_) => unreachable!("garbage bytes must not decode as a valid JPEG"),
        }
    }
}
