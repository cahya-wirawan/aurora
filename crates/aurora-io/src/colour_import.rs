//! Colour-managed import (colour management, round 1).
//!
//! Aurora's working space is encoded sRGB. A flat image that carries a valid
//! RGB ICC profile other than sRGB is converted into it when it is opened
//! ([`bring_into_working`]): relative colorimetric with black point
//! compensation ([`aurora_color::Transform::to_working`]), in `f32` on the
//! straight colour channels (alpha untouched, out-of-gamut colour clipped to 0..1), and the
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

/// The open-report line for a successful conversion from `description`'s
/// profile; `clips` says whether out-of-gamut colour was clipped (PSD
/// layers and flat images alike are). Kept in the document's
/// import losses, so it survives a `.aur` save and shows in a later Photoshop
/// save's report.
#[must_use]
pub fn converted_on_open_line(description: &str, clips: bool) -> String {
    format!(
        "The colours were converted from \"{description}\" to sRGB when the file was opened{}; \
         the original profile is not kept.",
        if clips {
            " (out-of-gamut colours are clipped)"
        } else {
            ""
        }
    )
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
        let is_gray_profile =
            parse_header(bytes).is_ok_and(|h| h.space == aurora_color::ProfileSpace::Gray);
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
    // Out-of-gamut results are clipped, as Photoshop clips a conversion
    // to sRGB (design owner, 2026-10-17): flat images and PSD layers alike.
    clamp_colour_unit(samples);
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

/// What a conversion staged: the converted colour of every pixel that is not
/// fully transparent, held aside until the whole image has converted, so a
/// failure part-way leaves the original untouched (all or nothing).
#[derive(Debug, Default)]
struct Staged {
    /// Pixel numbers (within the piece) of the pixels converted.
    kept: Vec<u32>,
    /// Their converted `r, g, b`, three per kept pixel.
    rgb: Vec<f16>,
}

/// A failure injected by a test, standing in for `lcms2` refusing a chunk.
fn injected_failure() -> aurora_color::ColorError {
    aurora_color::ColorError::BufferLengthMismatch {
        channels: aurora_core::Channels::Rgb,
        channel_count: 3,
        src_len: 0,
        dst_len: 1,
    }
}

/// Converts `samples` (read only) through `transform` into a [`Staged`].
/// `fail_chunk` makes that chunk fail (tests only; `None` in use).
fn stage_rgba(
    samples: &[f16],
    transform: &Transform,
    fail_chunk: Option<usize>,
) -> Result<Staged, aurora_color::ColorError> {
    let mut staged = Staged::default();
    if u32::try_from(samples.len() / 4).is_err() {
        return Err(injected_failure());
    }
    let mut rgb: Vec<f32> = Vec::with_capacity(CHUNK_PIXELS * 3);
    let mut converted: Vec<f32> = Vec::new();
    for (index, chunk) in samples.chunks(CHUNK_PIXELS * 4).enumerate() {
        if fail_chunk == Some(index) {
            return Err(injected_failure());
        }
        rgb.clear();
        let base = index * CHUNK_PIXELS;
        let first_kept = staged.kept.len();
        for (i, px) in chunk.chunks_exact(4).enumerate() {
            // Alpha zero (either sign): the colour cannot be seen.
            if px.get(3).is_some_and(|a| a.to_bits() << 1 == 0) {
                continue;
            }
            // Cannot fail: checked against `u32` above.
            if let Ok(n) = u32::try_from(base + i) {
                staged.kept.push(n);
            }
            for c in px.iter().take(3) {
                rgb.push(c.to_f32());
            }
        }
        if staged.kept.len() == first_kept {
            continue;
        }
        converted.clear();
        converted.resize(rgb.len(), 0.0);
        transform.transform(&rgb, &mut converted)?;
        staged
            .rgb
            .extend(converted.iter().map(|v| f16::from_f32(*v)));
    }
    Ok(staged)
}

/// Writes a [`Staged`] result into `samples` (the piece it was staged from).
fn commit_staged(samples: &mut [f16], staged: &Staged) {
    for (&pixel, rgb) in staged.kept.iter().zip(staged.rgb.chunks_exact(3)) {
        let at = pixel as usize * 4;
        if let Some(px) = samples.get_mut(at..at + 3) {
            px.copy_from_slice(rgb);
        }
    }
}

/// Converts the colour channels of straight RGBA `samples` through
/// `transform`, alpha untouched. Pixels that are fully transparent are
/// skipped (their colour is invisible and, in a layer image, they are most
/// of it); any partial trailing pixel is left alone.
///
/// **All or nothing:** the result is staged and written only if every chunk
/// converted, so an error leaves `samples` exactly as given.
///
/// # Errors
///
/// [`aurora_color::ColorError`] if `lcms2` refuses a chunk.
pub fn convert_rgba(
    samples: &mut [f16],
    transform: &Transform,
) -> Result<(), aurora_color::ColorError> {
    let staged = stage_rgba(samples, transform, None)?;
    commit_staged(samples, &staged);
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
/// transform; they share nothing). Identical to the single-threaded result
/// (every pixel is converted independently) and **all or nothing**: nothing
/// is written unless every part converted.
///
/// # Errors
///
/// [`aurora_color::ColorError`] if the profile cannot be used or a chunk is
/// refused; `samples` is then unchanged.
pub fn convert_profile_rgba(
    samples: &mut [f16],
    profile_bytes: &[u8],
    direction: ConvertDirection,
) -> Result<(), aurora_color::ColorError> {
    #[cfg(test)]
    let fail_chunk = TEST_FAIL_CHUNK.with(std::cell::Cell::get);
    #[cfg(not(test))]
    let fail_chunk = None;
    convert_profile_rgba_with(samples, profile_bytes, direction, fail_chunk)
}

#[cfg(test)]
thread_local! {
    /// Test hook: makes [`convert_profile_rgba`] fail in this chunk of its
    /// first part, on this thread only.
    pub(crate) static TEST_FAIL_CHUNK: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// [`convert_profile_rgba`] with a test-injected failure in chunk
/// `fail_chunk` of the first part.
pub(crate) fn convert_profile_rgba_with(
    samples: &mut [f16],
    profile_bytes: &[u8],
    direction: ConvertDirection,
    fail_chunk: Option<usize>,
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
        let staged = stage_rgba(samples, &build()?, fail_chunk)?;
        commit_staged(samples, &staged);
        return Ok(());
    }
    // Whole pixels per part.
    let part = ((samples.len() / threads).div_ceil(4) * 4).max(4);
    let staged: Vec<Result<Staged, aurora_color::ColorError>> = std::thread::scope(|scope| {
        let handles: Vec<_> = samples
            .chunks(part)
            .enumerate()
            .map(|(n, piece)| {
                scope.spawn(move || {
                    let transform = build()?;
                    stage_rgba(piece, &transform, if n == 0 { fail_chunk } else { None })
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err(injected_failure())))
            .collect()
    });
    let staged: Vec<Staged> = staged.into_iter().collect::<Result<_, _>>()?;
    for (piece, staged) in samples.chunks_mut(part).zip(&staged) {
        commit_staged(piece, staged);
    }
    Ok(())
}

/// Clamps the colour channels of straight RGBA `samples` to `0..=1` (alpha
/// untouched; NaN becomes 0). Used for PSD layers, whose blend modes,
/// adjustment tables and effects assume in-gamut colour: an out-of-gamut
/// conversion result is clipped the way Photoshop clips it when it converts
/// a document to sRGB.
pub fn clamp_colour_unit(samples: &mut [f16]) {
    for px in samples.chunks_exact_mut(4) {
        for c in px.iter_mut().take(3) {
            let v = c.to_f32();
            *c = f16::from_f32(if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        }
    }
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
