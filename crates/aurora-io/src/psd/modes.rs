//! Lab, Indexed Color and Bitmap pixels to straight `f16` sRGB for a PSD
//! open (task 148).
//!
//! Duotone and Multichannel files need no conversion code here: their first
//! channel is read as a grey plane, exactly like Grayscale (the open report
//! says what was left out).
//!
//! - **Lab** (modes 9, 8 and 16 bit): the stored planes are `L*`, `a*`,
//!   `b*` of CIE Lab relative to the D50 white. 8-bit: `L* = v / 255 * 100`,
//!   `a*, b* = v - 128`. 16-bit: `L* = v / 65535 * 100`,
//!   `a*, b* = (v - 32768) / 256` (checked against the RGB twin of
//!   psd-tools' `4x4_16bit_lab.psd`: its pixel 0 is `(10, 0, 178)`, whose
//!   Lab is `(19.08, 52.30, -85.05)` and whose stored 16-bit samples decode
//!   to `(19.07, 52.29, -85.07)`). The conversion is [`super::lab`]'s
//!   `Lab -> XYZ (D50) -> linear sRGB -> encoded sRGB` with every channel
//!   clipped to `0..=1`, the same maths that reproduces Photoshop's Photo
//!   Filter presets. sRGB's black point is zero, so black point
//!   compensation would change nothing.
//! - **Indexed Color** (mode 2, 8 bit): the colour mode data section holds
//!   the colour table, 256 reds, then 256 greens, then 256 blues (768
//!   bytes); each sample is an index into it. Image resource 1047, when
//!   present, names the transparent index.
//! - **Bitmap** (mode 0, 1 bit): rows of packed bits, most significant
//!   first, each row padded to a whole byte; a set bit is black.
//!
//! No 8-bit intermediate buffer exists: samples are read straight into the
//! `f16` output.

use half::f16;

use super::{malformed, opaque_buffer, promote_u8_table, try_alloc};
use crate::error::IoError;

/// The colour table of an Indexed Color file: three planes of 256 bytes.
pub(super) const PALETTE_BYTES: usize = 768;

/// Pixels worth a thread of their own.
const PARALLEL_MIN_PIXELS: usize = 1 << 18;

/// An Indexed Color file's table, as `f16` sRGB.
#[derive(Clone, Debug)]
pub(super) struct Palette {
    colours: Vec<[f16; 3]>,
    /// The index image resource 1047 marks transparent.
    transparent: Option<u8>,
}

impl Palette {
    /// Reads the 768-byte table; `transparent` is resource 1047's value.
    ///
    /// # Errors
    ///
    /// [`IoError::PsdMalformed`] when the section is not exactly 768 bytes.
    pub(super) fn parse(section: &[u8], transparent: Option<u16>) -> Result<Self, IoError> {
        if section.len() != PALETTE_BYTES {
            return Err(malformed(
                "an Indexed Color file whose colour table is not 768 bytes",
            ));
        }
        let table = promote_u8_table();
        let plane = |n: usize| section.get(n * 256..n * 256 + 256).unwrap_or_default();
        let (reds, greens, blues) = (plane(0), plane(1), plane(2));
        let at = |v: u8| table.get(usize::from(v)).copied().unwrap_or(f16::ZERO);
        let colours = reds
            .iter()
            .zip(greens)
            .zip(blues)
            .map(|((r, g), b)| [at(*r), at(*g), at(*b)])
            .collect();
        Ok(Self {
            colours,
            transparent: transparent.and_then(|v| u8::try_from(v).ok()),
        })
    }

    /// The straight RGBA of index `v`: transparent black for the
    /// transparent index.
    fn rgba(&self, v: u8) -> [f16; 4] {
        if self.transparent == Some(v) {
            return [f16::ZERO; 4];
        }
        let [r, g, b] = self
            .colours
            .get(usize::from(v))
            .copied()
            .unwrap_or([f16::ZERO; 3]);
        [r, g, b, f16::ONE]
    }
}

/// An Indexed Color plane (`pixels` index samples) as straight RGBA.
///
/// # Errors
///
/// [`IoError::PsdMalformed`] if the plane is shorter than `pixels`;
/// [`IoError::PsdOutOfMemory`] if the output cannot be allocated.
pub(super) fn indexed_to_rgba(
    index: &[u8],
    pixels: usize,
    palette: &Palette,
) -> Result<Vec<f16>, IoError> {
    let short = || malformed("an Indexed Color plane shorter than its image");
    let samples = index.get(..pixels).ok_or_else(short)?;
    let len = pixels.checked_mul(4).ok_or_else(short)?;
    let mut out: Vec<f16> = try_alloc(len)?;
    for v in samples {
        out.extend_from_slice(&palette.rgba(*v));
    }
    Ok(out)
}

/// A Bitmap plane (`height` rows of `(width + 7) / 8` bytes) as straight
/// opaque RGBA, a set bit black.
///
/// # Errors
///
/// [`IoError::PsdMalformed`] if the plane is shorter than its rows;
/// [`IoError::PsdOutOfMemory`] if the output cannot be allocated.
pub(super) fn bitmap_to_rgba(
    plane: &[u8],
    width: usize,
    height: usize,
) -> Result<Vec<f16>, IoError> {
    let short = || malformed("a Bitmap plane shorter than its image");
    let row_bytes = width.div_ceil(8);
    let need = row_bytes.checked_mul(height).ok_or_else(short)?;
    if plane.len() < need {
        return Err(short());
    }
    let mut out = opaque_buffer(width, height)?;
    for (y, row) in plane.chunks_exact(row_bytes.max(1)).take(height).enumerate() {
        for x in 0..width {
            let bit = row
                .get(x / 8)
                .is_some_and(|byte| byte >> (7 - x % 8) & 1 == 1);
            if !bit {
                let at = (y * width + x) * 4;
                if let Some(px) = out.get_mut(at..at + 3) {
                    px.fill(f16::ONE);
                }
            }
        }
    }
    Ok(out)
}

/// `L*`, `a*`, `b*` of the stored sample `v` at `bps` bytes: see the module
/// documentation for the scaling.
fn lab_value(channel: usize, plane: Option<&[u8]>, i: usize, bps: usize) -> f64 {
    let raw = plane.map(|p| {
        if bps == 2 {
            match p.get(i * 2..i * 2 + 2) {
                Some([hi, lo]) => f64::from(u16::from_be_bytes([*hi, *lo])),
                _ => 0.0,
            }
        } else {
            f64::from(p.get(i).copied().unwrap_or(0))
        }
    });
    let (full, neutral, span) = if bps == 2 {
        (65_535.0, 32_768.0, 256.0)
    } else {
        (255.0, 128.0, 1.0)
    };
    match (channel, raw) {
        // A missing lightness plane is white, a missing a* or b* neutral.
        (0, None) => 100.0,
        (0, Some(v)) => v / full * 100.0,
        (_, None) => 0.0,
        (_, Some(v)) => (v - neutral) / span,
    }
}

/// Converts `pixels` pixels of Lab `planes` (`L*`, `a*`, `b*` stored
/// samples of `bps` bytes, big-endian) with an optional alpha plane into
/// straight `f16` sRGB RGBA.
///
/// A pixel whose alpha is zero is not converted. The work is split over up
/// to 8 threads for a big image; the result does not depend on the thread
/// count.
///
/// # Errors
///
/// [`IoError::PsdMalformed`] if a plane is shorter than `pixels` samples;
/// [`IoError::PsdOutOfMemory`] if the output cannot be allocated.
pub(super) fn lab_to_rgba(
    planes: [Option<&[u8]>; 3],
    alpha: Option<&[u8]>,
    bps: usize,
    pixels: usize,
) -> Result<Vec<f16>, IoError> {
    let short = || malformed("a Lab plane shorter than its layer");
    let need = pixels.checked_mul(bps).ok_or_else(short)?;
    if planes.iter().flatten().any(|p| p.len() < need) || alpha.is_some_and(|a| a.len() < need) {
        return Err(short());
    }
    let len = pixels.checked_mul(4).ok_or_else(short)?;
    let mut out: Vec<f16> = try_alloc(len)?;
    out.resize(len, f16::ZERO);
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8);
    if threads <= 1 || pixels < PARALLEL_MIN_PIXELS {
        lab_range(&mut out, planes, alpha, bps, 0);
        return Ok(out);
    }
    let part = pixels.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (n, piece) in out.chunks_mut(part * 4).enumerate() {
            scope.spawn(move || lab_range(piece, planes, alpha, bps, n * part));
        }
    });
    Ok(out)
}

/// Converts the pixels `first ..` that `out` (4 samples a pixel) covers.
fn lab_range(
    out: &mut [f16],
    planes: [Option<&[u8]>; 3],
    alpha: Option<&[u8]>,
    bps: usize,
    first: usize,
) {
    let [l_plane, a_plane, b_plane] = planes;
    // A pixel equal to the one before it reuses the result.
    let mut previous: Option<([f64; 3], [f16; 3])> = None;
    for (n, px) in out.chunks_exact_mut(4).enumerate() {
        let i = first + n;
        let a = alpha.map_or(1.0, |plane| lab_alpha(plane, i, bps).clamp(0.0, 1.0));
        if a <= 0.0 {
            continue;
        }
        let lab = [
            lab_value(0, l_plane, i, bps),
            lab_value(1, a_plane, i, bps),
            lab_value(2, b_plane, i, bps),
        ];
        let rgb = match previous {
            Some((last, rgb)) if last == lab => rgb,
            _ => {
                let [r, g, b] = super::lab::lab_to_srgb(lab[0], lab[1], lab[2]);
                let rgb = [f16::from_f32(r), f16::from_f32(g), f16::from_f32(b)];
                previous = Some((lab, rgb));
                rgb
            }
        };
        if let [pr, pg, pb, pa] = px {
            let [r, g, b] = rgb;
            (*pr, *pg, *pb) = (r, g, b);
            *pa = f16::from_f32(a);
        }
    }
}

/// An alpha sample as `0.0..=1.0`, promoted as every other plane is.
fn lab_alpha(plane: &[u8], i: usize, bps: usize) -> f32 {
    if bps == 2 {
        match plane.get(i * 2..i * 2 + 2) {
            Some([hi, lo]) => aurora_color::promote_u16(u16::from_be_bytes([*hi, *lo])),
            _ => 0.0,
        }
    } else {
        aurora_color::promote_u8(plane.get(i).copied().unwrap_or(0))
    }
}
