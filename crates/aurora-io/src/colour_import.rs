//! Colour-managed import (colour management, round 1).
//!
//! Aurora's working space is encoded sRGB. A flat image that carries a valid
//! RGB ICC profile other than sRGB is converted into it when it is opened
//! ([`bring_into_working`]): relative colorimetric with black point
//! compensation ([`aurora_color::Transform::to_working`]), in `f32` on the
//! straight colour channels (alpha untouched, extended range kept), and the
//! source profile is returned as [`SourceProfile`] so the document can keep
//! it as metadata. A profile that is sRGB for practical purposes
//! ([`aurora_color::SRGB_EQUIVALENCE_TOLERANCE`]) changes no pixel at all. A
//! profile that cannot describe RGB pixels (wrong colour space or class,
//! too large, damaged) is ignored, never fatal, with a line in the
//! [`ColourReport`]. An untagged file is assumed sRGB, with an info line.
//!
//! **Not done:** grayscale ICC profiles on grayscale data (the profile is
//! reported as not applied), CMYK/Lab data, rendering-intent choices, and
//! converting in parallel (a large profiled image is converted on one
//! thread).

use std::path::Path;

use aurora_color::{IccProfile, Transform, inspect_rgb_source, parse_header};
use half::f16;

use crate::error::IoError;
use crate::image::Image;

/// How a file tagged its colours, before any judging.
#[derive(Debug, Default)]
pub(crate) struct SourceTag {
    /// The raw embedded ICC profile, if any.
    pub(crate) icc: Option<Vec<u8>>,
    /// PNG's `sRGB` chunk: tagged sRGB with no profile body.
    pub(crate) srgb_chunk: bool,
    /// Whether the source was single-channel grey (expanded to RGB here).
    pub(crate) gray: bool,
}

/// A decoded image exactly as stored, with its unjudged colour tag.
#[derive(Debug)]
pub(crate) struct ImageParts {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) samples: Vec<f16>,
    pub(crate) tag: SourceTag,
}

impl ImageParts {
    /// The long-standing raw reading: the embedded profile (strictly
    /// parsed) becomes the image's colour space, the pixels untouched.
    pub(crate) fn into_image(self) -> Result<Image, IoError> {
        let color_space = match &self.tag.icc {
            Some(bytes) => IccProfile::from_bytes(bytes)?,
            None => IccProfile::srgb(),
        };
        Image::new(self.width, self.height, color_space, self.samples)
    }
}

/// The source profile a document keeps as metadata: validated RGB profile
/// bytes and the profile's own name. The pixels are not in this space; they
/// were converted into the working space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceProfile {
    pub bytes: Vec<u8>,
    pub description: String,
}

/// What an import did about colour, in two levels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColourReport {
    /// Calm facts (assumed sRGB, converted from X).
    pub info: Vec<String>,
    /// Things the user should know were not honoured.
    pub warnings: Vec<String>,
}

/// [`bring_into_working`]'s result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColourImport {
    /// The profile the pixels came in, when it was not sRGB.
    pub source: Option<SourceProfile>,
    pub report: ColourReport,
}

/// How the file tagged its colours.
#[derive(Clone, Copy, Debug)]
pub enum Tagging<'a> {
    /// No profile and no sRGB tag.
    Untagged,
    /// Tagged sRGB without a profile body (a PNG `sRGB` chunk).
    SrgbTagged,
    /// An embedded ICC profile's bytes.
    Icc(&'a [u8]),
}

/// Pixels converted per `lcms2` call.
const CHUNK_PIXELS: usize = 16_384;

/// Converts `samples` (straight RGBA `f16`, in the colour space `tagging`
/// names) into the working space in place and reports what happened. See
/// the module documentation for the policy.
pub fn bring_into_working(samples: &mut [f16], tagging: Tagging<'_>, gray: bool) -> ColourImport {
    let mut out = ColourImport::default();
    let bytes = match tagging {
        Tagging::SrgbTagged => return out,
        Tagging::Untagged => {
            out.report
                .info
                .push("No colour profile: assumed sRGB.".to_owned());
            return out;
        }
        Tagging::Icc(bytes) => bytes,
    };
    let ignored = |why: String| {
        format!(
            "The embedded colour profile was not used ({why}); the colours are treated as sRGB."
        )
    };
    if gray {
        // A grey image's profile is grey (or a mismatched RGB one): not
        // applied in round 1.
        let named = aurora_color::profile_description(bytes);
        let is_gray_profile = parse_header(bytes)
            .map(|h| h.space == aurora_color::ProfileSpace::Gray)
            .unwrap_or(false);
        if is_gray_profile {
            out.report.warnings.push(ignored(format!(
                "grayscale profiles are not applied yet{}",
                named.map_or(String::new(), |n| format!(": \"{n}\""))
            )));
        }
        return out;
    }
    let source = match inspect_rgb_source(bytes) {
        Ok(source) => source,
        Err(problem) => {
            out.report.warnings.push(ignored(problem.describe()));
            return out;
        }
    };
    if source.equivalent_to_srgb {
        return out;
    }
    if convert_profile_rgba(samples, bytes, ConvertDirection::ToWorking).is_err() {
        out.report
            .warnings
            .push(ignored("the conversion failed".to_owned()));
        return out;
    }
    out.report.info.push(format!(
        "Converted from \"{}\" to sRGB.",
        source.description
    ));
    out.source = Some(SourceProfile {
        bytes: bytes.to_vec(),
        description: source.description,
    });
    out
}

/// Converts the colour channels of straight RGBA `samples` through
/// `transform` in place, alpha untouched. Pixels that are fully transparent
/// are skipped (their colour is invisible and, in a layer image, they are
/// most of it); any partial trailing pixel is left alone.
///
/// # Errors
///
/// [`aurora_color::ColorError`] if `lcms2` refuses a chunk (nothing in that
/// chunk is written).
pub fn convert_rgba(
    samples: &mut [f16],
    transform: &Transform,
) -> Result<(), aurora_color::ColorError> {
    let mut rgb: Vec<f32> = Vec::with_capacity(CHUNK_PIXELS * 3);
    let mut converted: Vec<f32> = Vec::new();
    let mut kept: Vec<usize> = Vec::with_capacity(CHUNK_PIXELS);
    for chunk in samples.chunks_mut(CHUNK_PIXELS * 4) {
        rgb.clear();
        kept.clear();
        let whole = chunk.len() / 4 * 4;
        let Some(pixels) = chunk.get_mut(..whole) else {
            continue;
        };
        for (i, px) in pixels.chunks_exact(4).enumerate() {
            // Alpha zero (either sign): the colour cannot be seen.
            if px.get(3).is_some_and(|a| a.to_bits() & 0x7fff == 0) {
                continue;
            }
            kept.push(i);
            for c in px.iter().take(3) {
                rgb.push(c.to_f32());
            }
        }
        if kept.is_empty() {
            continue;
        }
        converted.clear();
        converted.resize(rgb.len(), 0.0);
        transform.transform(&rgb, &mut converted)?;
        for (&i, out) in kept.iter().zip(converted.chunks_exact(3)) {
            if let Some(px) = pixels.get_mut(i * 4..i * 4 + 3) {
                for (dst, v) in px.iter_mut().zip(out) {
                    *dst = f16::from_f32(*v);
                }
            }
        }
    }
    Ok(())
}

/// Which way [`convert_profile_rgba`] converts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConvertDirection {
    /// From the profile into the sRGB working space.
    ToWorking,
    /// From the working space into the profile.
    FromWorking,
}

/// The fewest samples worth a thread of their own.
const PARALLEL_MIN_SAMPLES: usize = 1 << 20;

/// [`convert_rgba`] through the RGB profile `profile_bytes`, spread over the
/// machine's cores for a big image (each thread builds its own `lcms2`
/// transform; they share nothing). The result is identical to the single
/// threaded one: every pixel is converted independently.
///
/// # Errors
///
/// [`aurora_color::ColorError`] if the profile cannot be used or a chunk is
/// refused; on an error part of the image may already be converted, so a
/// caller that needs the original keeps a copy.
pub fn convert_profile_rgba(
    samples: &mut [f16],
    profile_bytes: &[u8],
    direction: ConvertDirection,
) -> Result<(), aurora_color::ColorError> {
    let build = || -> Result<Transform, aurora_color::ColorError> {
        let profile = IccProfile::from_bytes(profile_bytes)?;
        match direction {
            ConvertDirection::ToWorking => Transform::to_working(&profile),
            ConvertDirection::FromWorking => Transform::from_working(&profile),
        }
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8);
    if threads <= 1 || samples.len() < PARALLEL_MIN_SAMPLES {
        return convert_rgba(samples, &build()?);
    }
    // Whole pixels per part.
    let part = (samples.len() / threads).div_ceil(4) * 4;
    let results: Vec<Result<(), aurora_color::ColorError>> = std::thread::scope(|scope| {
        let handles: Vec<_> = samples
            .chunks_mut(part.max(4))
            .map(|piece| {
                scope.spawn(move || {
                    let transform = build()?;
                    convert_rgba(piece, &transform)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or(Err(aurora_color::ColorError::UnsupportedChannels(
                        aurora_core::Channels::Rgb,
                    )))
            })
            .collect()
    });
    results.into_iter().collect()
}

/// Decodes `bytes` by `path`'s extension into the working space
/// ([`bring_into_working`]); the returned [`Image`] is tagged sRGB and its
/// pixels are in it. A profile problem is a [`ColourReport`] warning, never
/// a failure.
///
/// # Errors
///
/// As [`crate::decode_by_extension`], minus profile errors.
pub fn decode_to_working(path: &Path, bytes: &[u8]) -> Result<(Image, ColourImport), IoError> {
    let extension = path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut parts = match extension.as_str() {
        "png" => crate::png::decode_parts(bytes)?,
        "jpg" | "jpeg" => crate::jpeg::decode_parts(bytes)?,
        "tif" | "tiff" => crate::tiff::decode_parts(bytes)?,
        _ => return Err(IoError::UnsupportedExtension(extension)),
    };
    let tagging = match (&parts.tag.icc, parts.tag.srgb_chunk) {
        (Some(icc), _) => Tagging::Icc(icc),
        (None, true) => Tagging::SrgbTagged,
        (None, false) => Tagging::Untagged,
    };
    let imported = bring_into_working(&mut parts.samples, tagging, parts.tag.gray);
    let image = Image::new(parts.width, parts.height, IccProfile::srgb(), parts.samples)?;
    Ok((image, imported))
}

#[cfg(test)]
#[path = "colour_import_tests.rs"]
mod tests;
