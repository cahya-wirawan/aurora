//! Mask feather (task 150): a Gaussian blur of a mask's coverage.
//!
//! A PSD layer or vector mask can carry a *feather* in its parameter
//! block (`MaskParameters`). Photoshop blurs the mask with a Gaussian
//! whose standard deviation is the feather value in pixels.
//!
//! **Measured** on `layer_mask_data.psd`, layer "2" (a green fill under an
//! ellipse mask with feather 6.0 and density 230): along a column crossing
//! only that layer, Photoshop's merged image is matched by
//! `density * gaussian(mask, sigma) + (1 - density)` with a mean error of
//! 0.0038 (max 0.011) in coverage at sigma 6.0, 0.0025 at 6.2, against
//! 0.0129 at 5.5 and 0.0162 at 7.0 (`NumPy`, an exact Gaussian truncated at
//! 4 sigma, the mask padded with its default colour). So sigma is the
//! feather value; density is applied after the blur, as before.
//!
//! The blur runs over the mask's own plane grown by the kernel radius on
//! every side, the constant outside the plane (hidden or shown) feeding
//! the edges, so the result is exactly that constant beyond its bounds.
//! It is bounded: a feather over [`MAX_SIGMA`], a grown plane that does
//! not fit the pixel budget, or more than [`MAX_TAP_WORK`] multiply-adds
//! leaves the mask hard-edged and the caller reports it.

use aurora_core::Rect;
use half::f16;

/// The largest feather (sigma, pixels) applied; a bigger one is left hard.
pub(super) const MAX_SIGMA: f64 = 128.0;

/// The most multiply-adds one feather may cost (about a second).
pub(super) const MAX_TAP_WORK: u64 = 1_500_000_000;

/// Budget pixels charged per grown-plane pixel (two `f32` planes and the
/// `f16` result, like a raster's).
const CHARGE: u64 = 2;

/// The kernel radius for `sigma`: 3.5 sigma, at least 1. `sigma` is at most
/// [`MAX_SIGMA`] here, so the radius is small and positive.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn radius(sigma: f64) -> usize {
    (sigma * 3.5).ceil().max(1.0) as usize
}

/// The normalised Gaussian taps `-radius..=radius`.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn kernel(sigma: f64, radius: usize) -> Vec<f32> {
    let mut taps: Vec<f64> = (0..=2 * radius)
        .map(|i| {
            let d = i as f64 - radius as f64;
            (-0.5 * d * d / (sigma * sigma)).exp()
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    for tap in &mut taps {
        *tap /= sum;
    }
    taps.into_iter().map(|tap| tap as f32).collect()
}

/// Blurs the `bounds`-sized `plane` (row-major, `0..=1`), whose value
/// beyond `bounds` is the constant `outside`, with a Gaussian of standard
/// deviation `sigma` pixels. Returns the grown bounds and the new plane,
/// or `None` when nothing needs doing (empty plane, `sigma` not above
/// zero or not finite) or the work is over a bound (the caller keeps the
/// hard mask and reports it; `budget` is untouched then). The grown plane
/// is charged to `budget` (budget pixels).
pub(super) fn blur_plane(
    plane: &[f16],
    bounds: Rect,
    outside: f32,
    sigma: f64,
    budget: &mut u64,
) -> Option<(Rect, Vec<f16>)> {
    if !(sigma.is_finite() && sigma > 0.0) || bounds.width == 0 || bounds.height == 0 {
        return None;
    }
    if sigma > MAX_SIGMA {
        return None;
    }
    let pad = radius(sigma);
    let (width, height) = (bounds.width as usize, bounds.height as usize);
    if plane.len() != width.checked_mul(height)? {
        return None;
    }
    let grown_width = width.checked_add(pad.checked_mul(2)?)?;
    let grown_height = height.checked_add(pad.checked_mul(2)?)?;
    let area = u64::try_from(grown_width.checked_mul(grown_height)?).ok()?;
    let taps = u64::try_from(2 * pad + 1).ok()?;
    if area.checked_mul(CHARGE)? > *budget || area.checked_mul(taps)?.checked_mul(2)? > MAX_TAP_WORK
    {
        return None;
    }
    let weights = kernel(sigma, pad);
    // Horizontal pass over the source rows only: a row of the padding is
    // the constant `outside`, whose blur is itself.
    let mut rows: Vec<f32> = Vec::new();
    rows.try_reserve_exact(grown_width.checked_mul(height)?)
        .ok()?;
    for row in plane.chunks_exact(width) {
        for gx in 0..grown_width {
            // Source column of tap `j`: `gx + j - 2 * pad`.
            let mut acc = 0.0f32;
            for (j, weight) in weights.iter().enumerate() {
                let value = (gx + j)
                    .checked_sub(2 * pad)
                    .and_then(|sx| row.get(sx))
                    .map_or(outside, |c| c.to_f32());
                acc = weight.mul_add(value, acc);
            }
            rows.push(acc);
        }
    }
    let mut out: Vec<f16> = Vec::new();
    out.try_reserve_exact(grown_width.checked_mul(grown_height)?)
        .ok()?;
    for gy in 0..grown_height {
        for gx in 0..grown_width {
            let mut acc = 0.0f32;
            for (j, weight) in weights.iter().enumerate() {
                let value = (gy + j)
                    .checked_sub(2 * pad)
                    .filter(|sy| *sy < height)
                    .and_then(|sy| rows.get(sy * grown_width + gx))
                    .copied()
                    .unwrap_or(outside);
                acc = weight.mul_add(value, acc);
            }
            out.push(f16::from_f32(acc.clamp(0.0, 1.0)));
        }
    }
    *budget -= area.checked_mul(CHARGE)?;
    let pad_px = i64::try_from(pad).ok()?;
    let grown = Rect {
        x: bounds.x - pad_px,
        y: bounds.y - pad_px,
        width: u32::try_from(grown_width).ok()?,
        height: u32::try_from(grown_height).ok()?,
    };
    Some((grown, out))
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::float_cmp
)]
mod tests {
    use super::*;

    fn plane(values: &[f32]) -> Vec<f16> {
        values.iter().map(|v| f16::from_f32(*v)).collect()
    }

    #[test]
    fn an_edge_becomes_a_symmetric_ramp_around_half() {
        // A 40 x 40 plane: the left half hidden, the right half shown,
        // hidden beyond; read the middle row, far from the top and bottom.
        let v: Vec<f32> = (0..1600)
            .map(|i| if i % 40 < 20 { 0.0 } else { 1.0 })
            .collect();
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 40,
        };
        let mut budget = u64::MAX;
        let Some((grown, out)) = blur_plane(&plane(&v), bounds, 0.0, 3.0, &mut budget) else {
            unreachable!("blurs");
        };
        // radius = ceil(10.5) = 11 on each side.
        assert_eq!(
            (grown.x, grown.y, grown.width, grown.height),
            (-11, -11, 62, 62)
        );
        let mid = (11 + 20) * 62;
        let at = |x: i64| out[mid + (x + 11) as usize].to_f32();
        // The edge lies between source columns 19 and 20.
        assert!(
            (at(19) + at(20) - 1.0).abs() < 0.01,
            "symmetric about the edge"
        );
        assert!(at(19) > 0.3 && at(19) < 0.5, "{}", at(19));
        // Three sigma away the ramp is at its ends.
        assert!(at(11) < 0.03 && at(28) > 0.97, "{} {}", at(11), at(28));
        // Beyond the plane's own left edge everything is hidden and fading.
        assert!(at(-8) < 0.001, "{}", at(-8));
        // Charged to the budget.
        assert_eq!(budget, u64::MAX - 62 * 62 * CHARGE);
    }

    #[test]
    fn a_shown_plane_in_a_shown_surround_stays_shown_and_in_a_hidden_one_keeps_its_total() {
        let bounds = Rect {
            x: 5,
            y: 7,
            width: 9,
            height: 9,
        };
        let v = vec![1.0; 81];
        let mut budget = u64::MAX;
        let Some((_, out)) = blur_plane(&plane(&v), bounds, 0.0, 2.0, &mut budget) else {
            unreachable!("blurs");
        };
        let sum: f32 = out.iter().map(|c| c.to_f32()).sum();
        assert!((sum - 81.0).abs() < 1.0, "sum {sum}");
        assert!(out.iter().all(|c| c.to_f32() < 1.0));
        let Some((_, out)) = blur_plane(&plane(&v), bounds, 1.0, 2.0, &mut budget) else {
            unreachable!("blurs");
        };
        assert!(out.iter().all(|c| c.to_f32() == 1.0));
    }

    #[test]
    fn the_bounds_decline_what_they_must() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        };
        let v = vec![1.0; 16];
        let mut budget = u64::MAX;
        assert!(blur_plane(&plane(&v), bounds, 0.0, 0.0, &mut budget).is_none());
        assert!(blur_plane(&plane(&v), bounds, 0.0, f64::NAN, &mut budget).is_none());
        assert!(blur_plane(&plane(&v), bounds, 0.0, MAX_SIGMA + 1.0, &mut budget).is_none());
        let empty = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 4,
        };
        assert!(blur_plane(&[], empty, 0.0, 2.0, &mut budget).is_none());
        // Too small a pixel budget: declined and not charged.
        let mut small = 10;
        assert!(blur_plane(&plane(&v), bounds, 0.0, 2.0, &mut small).is_none());
        assert_eq!(small, 10);
        // A plane whose length disagrees with its bounds.
        assert!(blur_plane(&plane(&v[..15]), bounds, 0.0, 2.0, &mut budget).is_none());
    }
}
