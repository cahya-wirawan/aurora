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
//! reported as not applied), CMYK/Lab data and rendering-intent choices.

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

/// log2 of [`ColourCache`]'s slot count.
const CACHE_BITS: u32 = 14;
/// A slot no colour occupies (a key is 48 bits, so it never equals this).
const EMPTY_KEY: u64 = u64::MAX;
/// A slot's value is a miss index waiting for this chunk's `lcms2` call.
const PENDING: u64 = 1 << 63;

/// An exact memo of one transform's results: the three `f16` colour
/// samples of a pixel, as bits, to the three `f16` samples converting them
/// gives. `lcms2` evaluates a float transform pixel by pixel with nothing
/// carried between pixels (no cache, no dithering), so the same input bits
/// always give the same output bits, and the memo returns exactly what
/// converting the pixel again would. A PSD layer is mostly a few colours
/// (the Nyepi template's 114 M visible layer pixels hold about 110,000
/// distinct colours per conversion part), and per-pixel `lcms2` float
/// evaluation was where the open's time went.
///
/// Direct-mapped and never resized: a colour that collides with another
/// replaces it.
#[derive(Debug)]
struct ColourCache {
    /// `(key, value)`: the input bits (`r << 32 | g << 16 | b`) and either
    /// the output bits packed the same way or [`PENDING`] plus a miss index.
    slots: Vec<(u64, u64)>,
}

impl ColourCache {
    fn new() -> Self {
        Self {
            slots: vec![(EMPTY_KEY, 0); 1 << CACHE_BITS],
        }
    }

    /// Fibonacci hashing; the shift leaves `CACHE_BITS` bits, so the
    /// result always fits a `usize` and indexes `slots`.
    fn slot_of(key: u64) -> usize {
        #[allow(clippy::cast_possible_truncation)]
        let slot = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - CACHE_BITS)) as usize;
        slot
    }

    /// Forgets every slot still waiting for a result (after a failed call).
    fn forget_pending(&mut self) {
        for slot in &mut self.slots {
            if slot.1 & PENDING != 0 {
                *slot = (EMPTY_KEY, 0);
            }
        }
    }
}

fn pack_rgb(rgb: [u16; 3]) -> u64 {
    (u64::from(rgb[0]) << 32) | (u64::from(rgb[1]) << 16) | u64::from(rgb[2])
}

fn unpack_rgb(packed: u64) -> [f16; 3] {
    // Truncation intended: each field is 16 bits.
    #[allow(clippy::cast_possible_truncation)]
    let field = |shift: u32| f16::from_bits((packed >> shift) as u16);
    [field(32), field(16), field(0)]
}

/// One thread's means of converting: a transform and its memo.
#[derive(Debug)]
struct Worker {
    transform: Transform,
    cache: ColourCache,
}

/// Converts `samples` (read only) through `transform` into a [`Staged`],
/// memoising in `cache` (which must only ever have seen `transform`'s
/// results, or one built identically). `fail_chunk` makes that chunk fail
/// (tests only; `None` in use).
///
/// Each chunk's distinct colours that are not memoised go to `lcms2` in one
/// call; every other kept pixel is copied from the memo. The result is bit
/// for bit what converting every kept pixel would give
/// (`the_memoised_conversion_equals_converting_every_pixel`).
fn stage_rgba(
    samples: &[f16],
    transform: &Transform,
    cache: &mut ColourCache,
    fail_chunk: Option<usize>,
) -> Result<Staged, aurora_color::ColorError> {
    let mut staged = Staged::default();
    if u32::try_from(samples.len() / 4).is_err() {
        return Err(injected_failure());
    }
    // The distinct missing colours of a chunk (`f32`, three per miss), the
    // memo slot each claimed, and where each kept pixel's result goes.
    let mut misses: Vec<f32> = Vec::with_capacity(CHUNK_PIXELS * 3);
    let mut miss_slots: Vec<Option<usize>> = Vec::new();
    let mut fixups: Vec<(usize, usize)> = Vec::new();
    let mut converted: Vec<f32> = Vec::new();
    let mut converted_bits: Vec<u64> = Vec::new();
    for (index, chunk) in samples.chunks(CHUNK_PIXELS * 4).enumerate() {
        misses.clear();
        miss_slots.clear();
        fixups.clear();
        let base = index * CHUNK_PIXELS;
        for (i, px) in chunk.chunks_exact(4).enumerate() {
            let [r, g, b, a] = [0, 1, 2, 3].map(|k| px.get(k).map_or(0, |v| v.to_bits()));
            // Alpha zero (either sign): the colour cannot be seen.
            if a << 1 == 0 {
                continue;
            }
            // Cannot fail: checked against `u32` above.
            if let Ok(n) = u32::try_from(base + i) {
                staged.kept.push(n);
            }
            let key = pack_rgb([r, g, b]);
            let slot_at = ColourCache::slot_of(key);
            let slot = cache.slots.get(slot_at).copied();
            match slot {
                Some((held, value)) if held == key && value & PENDING == 0 => {
                    staged.rgb.extend(unpack_rgb(value));
                }
                Some((held, value)) if held == key => {
                    // A colour already missed in this chunk. The index is
                    // below `CHUNK_PIXELS`, so it fits.
                    #[allow(clippy::cast_possible_truncation)]
                    fixups.push((staged.rgb.len(), (value & !PENDING) as usize));
                    staged.rgb.extend([f16::ZERO; 3]);
                }
                _ => {
                    let miss = miss_slots.len();
                    // A slot another of this chunk's misses holds stays
                    // with it; this colour is converted unmemoised.
                    let claimable = slot.is_some_and(|(_, value)| value & PENDING == 0);
                    match cache.slots.get_mut(slot_at) {
                        Some(entry) if claimable => {
                            *entry = (key, PENDING | miss as u64);
                            miss_slots.push(Some(slot_at));
                        }
                        _ => miss_slots.push(None),
                    }
                    misses.extend([r, g, b].map(|c| f16::from_bits(c).to_f32()));
                    fixups.push((staged.rgb.len(), miss));
                    staged.rgb.extend([f16::ZERO; 3]);
                }
            }
        }
        // A test's injected failure stands where `lcms2` would refuse the
        // chunk: after it has claimed its memo slots.
        if fail_chunk == Some(index) {
            cache.forget_pending();
            return Err(injected_failure());
        }
        if misses.is_empty() {
            continue;
        }
        converted.clear();
        converted.resize(misses.len(), 0.0);
        if let Err(err) = transform.transform(&misses, &mut converted) {
            cache.forget_pending();
            return Err(err);
        }
        converted_bits.clear();
        converted_bits.extend(converted.chunks_exact(3).map(|c| {
            pack_rgb([0, 1, 2].map(|k| c.get(k).map_or(0, |v| f16::from_f32(*v).to_bits())))
        }));
        for (slot_at, bits) in miss_slots.iter().zip(&converted_bits) {
            if let Some(entry) = slot_at.and_then(|at| cache.slots.get_mut(at)) {
                entry.1 = *bits;
            }
        }
        for &(at, miss) in &fixups {
            if let (Some(out), Some(bits)) =
                (staged.rgb.get_mut(at..at + 3), converted_bits.get(miss))
            {
                out.copy_from_slice(&unpack_rgb(*bits));
            }
        }
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
    let staged = stage_rgba(samples, transform, &mut ColourCache::new(), None)?;
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

/// Converts many images through one RGB profile ([`convert_profile_rgba`]
/// is one image through a fresh one): it keeps the `lcms2` transforms it
/// builds (one per thread that has needed one, each with its memo of
/// converted colours) for the next image, so a PSD open builds a handful of
/// transforms instead of several per layer. Every transform is built from
/// the same bytes the same way, so which one converts a pixel never changes
/// its result.
#[derive(Debug)]
pub struct ProfileConverter {
    profile_bytes: Vec<u8>,
    direction: ConvertDirection,
    /// Idle workers; a thread takes one (or builds one) and gives it back.
    idle: std::sync::Mutex<Vec<Worker>>,
}

impl ProfileConverter {
    /// A converter through the RGB profile `profile_bytes`; nothing is built
    /// until the first conversion.
    #[must_use]
    pub fn new(profile_bytes: Vec<u8>, direction: ConvertDirection) -> Self {
        Self {
            profile_bytes,
            direction,
            idle: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn take(&self) -> Result<Worker, aurora_color::ColorError> {
        // A poisoned pool only means another thread panicked holding the
        // lock; its workers are not trusted and a new one is built.
        if let Some(worker) = self.idle.lock().ok().and_then(|mut idle| idle.pop()) {
            return Ok(worker);
        }
        let profile = IccProfile::from_bytes(&self.profile_bytes)?;
        let transform = match self.direction {
            ConvertDirection::ToWorking => Transform::to_working(&profile),
            ConvertDirection::FromWorking => Transform::from_working(&profile),
        }?;
        Ok(Worker {
            transform,
            cache: ColourCache::new(),
        })
    }

    fn give(&self, worker: Worker) {
        if let Ok(mut idle) = self.idle.lock() {
            idle.push(worker);
        }
    }

    /// Stages `piece` with an idle (or new) worker, giving it back after.
    fn stage(
        &self,
        piece: &[f16],
        fail_chunk: Option<usize>,
    ) -> Result<Staged, aurora_color::ColorError> {
        let mut worker = self.take()?;
        let staged = stage_rgba(piece, &worker.transform, &mut worker.cache, fail_chunk);
        self.give(worker);
        staged
    }

    /// [`convert_profile_rgba`] through this converter.
    ///
    /// # Errors
    ///
    /// As [`convert_profile_rgba`]; `samples` is then unchanged.
    pub fn convert(&self, samples: &mut [f16]) -> Result<(), aurora_color::ColorError> {
        #[cfg(test)]
        let fail_chunk = TEST_FAIL_CHUNK.with(std::cell::Cell::get);
        #[cfg(not(test))]
        let fail_chunk = None;
        self.convert_with(samples, fail_chunk)
    }

    /// [`Self::convert`] with a test-injected failure in chunk `fail_chunk`
    /// of the first part.
    pub(crate) fn convert_with(
        &self,
        samples: &mut [f16],
        fail_chunk: Option<usize>,
    ) -> Result<(), aurora_color::ColorError> {
        let threads = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(8);
        if threads <= 1 || samples.len() < PARALLEL_MIN_SAMPLES {
            let staged = self.stage(samples, fail_chunk)?;
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
                    scope.spawn(move || self.stage(piece, if n == 0 { fail_chunk } else { None }))
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
}

/// [`convert_rgba`] through the RGB profile `profile_bytes`, spread over the
/// machine's cores for a big image (each thread uses its own `lcms2`
/// transform; they share nothing). Identical to the single-threaded result
/// (every pixel is converted independently) and **all or nothing**: nothing
/// is written unless every part converted. Converting several images
/// through one profile is cheaper with one [`ProfileConverter`].
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
    ProfileConverter::new(profile_bytes.to_vec(), direction).convert(samples)
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
#[cfg(test)]
pub(crate) fn convert_profile_rgba_with(
    samples: &mut [f16],
    profile_bytes: &[u8],
    direction: ConvertDirection,
    fail_chunk: Option<usize>,
) -> Result<(), aurora_color::ColorError> {
    ProfileConverter::new(profile_bytes.to_vec(), direction).convert_with(samples, fail_chunk)
}

/// The conversion as it was before the memo (0.276.0), kept as the
/// reference the memoised one is tested against: every kept pixel goes
/// through `lcms2`.
#[cfg(test)]
pub(crate) fn convert_rgba_unmemoised(
    samples: &mut [f16],
    transform: &Transform,
) -> Result<(), aurora_color::ColorError> {
    let mut staged = Staged::default();
    let mut rgb: Vec<f32> = Vec::with_capacity(CHUNK_PIXELS * 3);
    let mut converted: Vec<f32> = Vec::new();
    for (index, chunk) in samples.chunks(CHUNK_PIXELS * 4).enumerate() {
        rgb.clear();
        let base = index * CHUNK_PIXELS;
        let first_kept = staged.kept.len();
        for (i, px) in chunk.chunks_exact(4).enumerate() {
            if px.get(3).is_some_and(|a| a.to_bits() << 1 == 0) {
                continue;
            }
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
    commit_staged(samples, &staged);
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
            // `+0.0..=1.0` (bits `0x0000..=0x3C00`) is left as it is: the
            // clamp returns it and `f16 -> f32 -> f16` is exact for it, so
            // skipping the round trip changes no bit.
            if c.to_bits() > ONE_BITS {
                let v = c.to_f32();
                *c = f16::from_f32(if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
            }
        }
    }
}

/// `f16` 1.0's bits; every `f16` from `+0.0` to `1.0` has bits at most this.
const ONE_BITS: u16 = 0x3C00;

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
