//! CMYK pixels to the working space for a PSD open (task 146).
//!
//! A CMYK layer or merged image arrives as four planes of stored samples
//! (and perhaps an alpha plane); Photoshop stores CMYK **inverted** (the
//! largest value is no ink), so the ink coverage of a sample `v` (as a
//! fraction of full scale) is `1 - v`. [`CmykConverter::convert_planes`]
//! turns the planes into straight `f16` RGBA tagged for the working space
//! (encoded sRGB), clipped to `0..=1` like every PSD layer colour:
//!
//! - with the file's embedded CMYK profile, through `lcms2` (relative
//!   colorimetric with black point compensation, like the RGB conversion);
//! - without one (or if it cannot be used), the naive formula
//!   `rgb = (1 - ink) * (1 - black)`, which knows nothing of real inks and
//!   which the open report says is an assumption.
//!
//! The planes are read straight into `f32` ink values chunk by chunk (no
//! 8-bit intermediate and no `f16` CMYK buffer), pixels whose alpha is zero
//! are not converted (their colour cannot be seen; they are written as
//! transparent black), a pixel equal to the one before it reuses its
//! result, and a large image is converted on several threads, each with its
//! own transform. The result does not depend on the thread count.

use aurora_color::{CmykTransform, IccProfile, naive_cmyk_to_rgb, promote_u8, promote_u16};
use half::f16;

use crate::error::IoError;

/// Pixels converted per `lcms2` call.
const CHUNK_PIXELS: usize = 16_384;

/// The fewest pixels worth a thread of their own.
const PARALLEL_MIN_PIXELS: usize = 1 << 18;

/// A CMYK to working-space converter: one profile (or none), one transform
/// per thread that has needed one, kept for the next image.
#[derive(Debug)]
pub(crate) struct CmykConverter {
    /// The embedded profile's bytes, or `None` for the naive formula.
    profile: Option<Vec<u8>>,
    idle: std::sync::Mutex<Vec<CmykTransform>>,
}

impl CmykConverter {
    pub(crate) fn new(profile: Option<Vec<u8>>) -> Self {
        Self {
            profile,
            idle: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// One colour: `ink` (cyan, magenta, yellow, black, each `0..=1`) as
    /// encoded sRGB in `0..=1`. For a descriptor colour of a fill layer.
    pub(crate) fn colour(&self, ink: [f32; 4]) -> [f32; 3] {
        let mut rgb = [0.0_f32; 3];
        let converted = self.take().is_some_and(|t| {
            let ok = t.transform(&ink, &mut rgb).is_ok();
            self.give(t);
            ok
        });
        if !converted {
            rgb = naive_cmyk_to_rgb(ink);
        }
        rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) })
    }

    fn take(&self) -> Option<CmykTransform> {
        if self.profile.is_none() {
            return None;
        }
        if let Some(t) = self.idle.lock().ok().and_then(|mut idle| idle.pop()) {
            return Some(t);
        }
        let bytes = self.profile.as_deref()?;
        let profile = IccProfile::from_bytes(bytes).ok()?;
        CmykTransform::to_working(&profile).ok()
    }

    fn give(&self, t: CmykTransform) {
        if let Ok(mut idle) = self.idle.lock() {
            idle.push(t);
        }
    }

    /// Converts `pixels` pixels of `planes` (cyan, magenta, yellow, black
    /// stored samples, `bps` bytes each, big-endian; a missing one is no ink)
    /// with an optional
    /// `alpha` plane into straight RGBA `f16`.
    ///
    /// # Errors
    ///
    /// [`IoError::PsdOutOfMemory`] if the output cannot be allocated;
    /// [`IoError::PsdMalformed`] if a plane is shorter than `pixels`
    /// samples.
    pub(crate) fn convert_planes(
        &self,
        planes: [Option<&[u8]>; 4],
        alpha: Option<&[u8]>,
        bps: usize,
        pixels: usize,
    ) -> Result<Vec<f16>, IoError> {
        let short = || IoError::PsdMalformed {
            what: "a CMYK plane shorter than its layer",
        };
        let need = pixels.checked_mul(bps).ok_or_else(short)?;
        if planes.iter().flatten().any(|p| p.len() < need) || alpha.is_some_and(|a| a.len() < need) {
            return Err(short());
        }
        let len = pixels.checked_mul(4).ok_or_else(short)?;
        let mut out: Vec<f16> = Vec::new();
        out.try_reserve_exact(len)
            .map_err(|_| IoError::PsdOutOfMemory {
                bytes: (len as u64).saturating_mul(2),
            })?;
        out.resize(len, f16::ZERO);
        let threads = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(8);
        if threads <= 1 || pixels < PARALLEL_MIN_PIXELS {
            self.convert_range(&mut out, planes, alpha, bps, 0);
            return Ok(out);
        }
        let part = pixels.div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for (n, piece) in out.chunks_mut(part * 4).enumerate() {
                scope.spawn(move || {
                    self.convert_range(piece, planes, alpha, bps, n * part);
                });
            }
        });
        Ok(out)
    }

    /// Converts the pixels `first ..` that `out` (4 samples a pixel) covers.
    fn convert_range(
        &self,
        out: &mut [f16],
        planes: [Option<&[u8]>; 4],
        alpha: Option<&[u8]>,
        bps: usize,
        first: usize,
    ) {
        let transform = self.take();
        let sample = |plane: &[u8], i: usize| -> f32 {
            if bps == 2 {
                match plane.get(i * 2..i * 2 + 2) {
                    Some([hi, lo]) => promote_u16(u16::from_be_bytes([*hi, *lo])),
                    _ => 0.0,
                }
            } else {
                promote_u8(plane.get(i).copied().unwrap_or(0))
            }
        };
        let mut inks: Vec<f32> = Vec::with_capacity(CHUNK_PIXELS * 4);
        let mut rgb: Vec<f32> = Vec::new();
        // Per pixel of the chunk: `None` = transparent, else the index of
        // its converted colour among the distinct-from-previous ones.
        let mut slot: Vec<Option<usize>> = Vec::with_capacity(CHUNK_PIXELS);
        let mut alphas: Vec<f32> = Vec::with_capacity(CHUNK_PIXELS);
        for (c, chunk) in out.chunks_mut(CHUNK_PIXELS * 4).enumerate() {
            inks.clear();
            slot.clear();
            alphas.clear();
            let base = first + c * CHUNK_PIXELS;
            let mut previous: Option<[f32; 4]> = None;
            for i in 0..chunk.len() / 4 {
                let a = alpha.map_or(1.0, |p| sample(p, base + i));
                alphas.push(a);
                if a <= 0.0 {
                    slot.push(None);
                    continue;
                }
                let ink = [0, 1, 2, 3].map(|k| {
                    let v = planes
                        .get(k)
                        .copied()
                        .flatten()
                        .map_or(1.0, |p| sample(p, base + i));
                    1.0 - v
                });
                if previous != Some(ink) {
                    inks.extend_from_slice(&ink);
                    previous = Some(ink);
                }
                slot.push(Some((inks.len() / 4).saturating_sub(1)));
            }
            rgb.clear();
            rgb.resize(inks.len() / 4 * 3, 0.0);
            let converted = transform.as_ref().is_some_and(|t| {
                let ok = t.transform(&inks, &mut rgb).is_ok();
                if !ok {
                    tracing::warn!("a CMYK chunk failed to convert; the formula is used for it");
                }
                ok
            });
            if !converted {
                for (ink, out_rgb) in inks.chunks_exact(4).zip(rgb.chunks_exact_mut(3)) {
                    if let ([c, m, y, k], [r, g, b]) = (ink, out_rgb) {
                        [*r, *g, *b] = naive_cmyk_to_rgb([*c, *m, *y, *k]);
                    }
                }
            }
            for ((px, s), a) in chunk.chunks_exact_mut(4).zip(&slot).zip(&alphas) {
                let colour = s
                    .and_then(|s| rgb.get(s * 3..s * 3 + 3))
                    .map_or([0.0; 3], |c| {
                        [0, 1, 2].map(|k| c.get(k).copied().unwrap_or(0.0))
                    });
                for (dst, v) in px.iter_mut().zip(colour) {
                    *dst = f16::from_f32(if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
                }
                if let Some(dst) = px.get_mut(3) {
                    *dst = f16::from_f32(a.clamp(0.0, 1.0));
                }
            }
        }
        if let Some(t) = transform {
            self.give(t);
        }
    }
}
