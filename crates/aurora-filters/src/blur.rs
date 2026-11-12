//! Gaussian blur of a premultiplied RGBA window (task 176).
//!
//! Exact integer arithmetic on purpose, for the reason [`crate::shadow`]
//! gives: every sample is held as Q24 fixed point (`i64`, `1.0 = 2^24`,
//! finer than `f16`'s 11-bit mantissa) and every kernel weight or box sum
//! is an exact integer, so a pixel's result is a function of the source
//! values around it only, never of where the window starts. Two composite
//! tiles whose windows overlap compute bit-identical values where they
//! meet: a blurred layer is seamless across tile boundaries. A running
//! `f32` sum would not be.
//!
//! What is computed is [`aurora_core::GaussianSpec`]: the exact truncated
//! Gaussian for radii up to 4 px, three box blurs above that. Samples
//! outside the window count as transparent; the result is exact wherever
//! the window's margin is at least the spec's [`reach`].
//!
//! [`reach`]: aurora_core::GaussianSpec::reach

use aurora_core::GaussianSpec;

/// `1.0` in the Q24 fixed point this module computes in.
const ONE: f64 = (1_u64 << 24) as f64;

/// Largest magnitude a sample may take (`f16`'s maximum, 65504, is below
/// it): keeps every sum far inside `i64`.
const SAMPLE_LIMIT: f32 = 65_536.0;

const CHANNELS: usize = 4;

/// Blurs a `side` x `side` window of premultiplied RGBA `f32` (row-major,
/// 4 values per pixel) in place with `spec`. A non-finite sample counts as
/// `0`. Does nothing when `window.len() != side * side * 4`.
///
/// The outer ring of the window, as wide as the spec's reach, is computed
/// from a truncated neighbourhood and is not exact; callers crop it away.
pub fn gaussian_blur_window(window: &mut [f32], side: usize, spec: &GaussianSpec) {
    let Some(pixels) = side.checked_mul(side) else {
        return;
    };
    if pixels.checked_mul(CHANNELS) != Some(window.len()) || side == 0 {
        return;
    }
    let mut plane = vec![0_i64; pixels];
    let mut line = vec![0_i64; side];
    let mut spare = vec![0_i64; side];
    let mut prefix = vec![0_i64; side + 1];
    for channel in 0..CHANNELS {
        for (dst, texel) in plane.iter_mut().zip(window.chunks_exact(CHANNELS)) {
            let v = texel.get(channel).copied().unwrap_or(0.0);
            *dst = to_fixed(v);
        }
        for axis in [Axis::Rows, Axis::Cols] {
            for k in 0..side {
                gather(&plane, side, axis, k, &mut line);
                match spec {
                    GaussianSpec::Exact { weights } => {
                        exact_line(&line, &mut spare, weights);
                        std::mem::swap(&mut line, &mut spare);
                    }
                    GaussianSpec::Boxes { radii } => {
                        for &r in radii {
                            box_line(&line, &mut spare, r as usize, &mut prefix);
                            std::mem::swap(&mut line, &mut spare);
                        }
                    }
                }
                scatter(&mut plane, side, axis, k, &line);
            }
        }
        for (texel, value) in window.chunks_exact_mut(CHANNELS).zip(&plane) {
            if let Some(slot) = texel.get_mut(channel) {
                *slot = from_fixed(*value);
            }
        }
    }
}

/// The centre `out` x `out` pixels of a `side` x `side` RGBA window
/// (`side - out` even), row-major. Empty when the sizes do not fit.
#[must_use]
pub fn crop_window(window: &[f32], side: usize, out: usize) -> Vec<f32> {
    let Some(margin) = side.checked_sub(out) else {
        return Vec::new();
    };
    if margin % 2 != 0
        || side.checked_mul(side).and_then(|n| n.checked_mul(CHANNELS)) != Some(window.len())
    {
        return Vec::new();
    }
    let margin = margin / 2;
    let mut result = Vec::with_capacity(out * out * CHANNELS);
    for y in 0..out {
        let start = ((y + margin) * side + margin) * CHANNELS;
        match window.get(start..start + out * CHANNELS) {
            Some(row) => result.extend_from_slice(row),
            None => return Vec::new(),
        }
    }
    result
}

#[derive(Clone, Copy)]
enum Axis {
    Rows,
    Cols,
}

#[allow(clippy::cast_possible_truncation)]
fn to_fixed(v: f32) -> i64 {
    if v.is_finite() {
        (f64::from(v.clamp(-SAMPLE_LIMIT, SAMPLE_LIMIT)) * ONE).round() as i64
    } else {
        0
    }
}

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn from_fixed(v: i64) -> f32 {
    (v as f64 / ONE) as f32
}

fn gather(plane: &[i64], side: usize, axis: Axis, k: usize, line: &mut [i64]) {
    for (j, slot) in line.iter_mut().enumerate() {
        let index = match axis {
            Axis::Rows => k * side + j,
            Axis::Cols => j * side + k,
        };
        *slot = plane.get(index).copied().unwrap_or(0);
    }
}

fn scatter(plane: &mut [i64], side: usize, axis: Axis, k: usize, line: &[i64]) {
    for (j, value) in line.iter().enumerate() {
        let index = match axis {
            Axis::Rows => k * side + j,
            Axis::Cols => j * side + k,
        };
        if let Some(slot) = plane.get_mut(index) {
            *slot = *value;
        }
    }
}

/// `out[j] = sum_k weights[k] * line[j + k - r]` in units of `1 / 2^16`,
/// rounded half up; outside the line is zero.
fn exact_line(line: &[i64], out: &mut [i64], weights: &[i64]) {
    let r = weights.len() / 2;
    let n = line.len();
    for (j, slot) in out.iter_mut().enumerate() {
        let lo = j.saturating_sub(r);
        let hi = (j + r).min(n.saturating_sub(1));
        let mut sum = 0_i64;
        for i in lo..=hi {
            let k = i + r - j;
            sum += weights.get(k).copied().unwrap_or(0) * line.get(i).copied().unwrap_or(0);
        }
        *slot = (sum + (aurora_core::filters::KERNEL_ONE >> 1)) >> 16;
    }
}

/// A box blur of radius `r` (width `2 r + 1`), rounded half up; outside
/// the line is zero. `prefix` is scratch of `line.len() + 1`.
fn box_line(line: &[i64], out: &mut [i64], r: usize, prefix: &mut [i64]) {
    let n = line.len();
    let mut acc = 0_i64;
    if let Some(first) = prefix.first_mut() {
        *first = 0;
    }
    for (i, v) in line.iter().enumerate() {
        acc += *v;
        if let Some(slot) = prefix.get_mut(i + 1) {
            *slot = acc;
        }
    }
    let width = i64::try_from(2 * r + 1).unwrap_or(i64::MAX);
    for (j, slot) in out.iter_mut().enumerate() {
        let lo = j.saturating_sub(r);
        let hi = (j + r + 1).min(n);
        let sum = prefix.get(hi).copied().unwrap_or(0) - prefix.get(lo).copied().unwrap_or(0);
        *slot = (sum + width / 2).div_euclid(width);
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]
mod tests {
    use super::*;

    fn window(side: usize, f: impl Fn(usize, usize) -> [f32; 4]) -> Vec<f32> {
        let mut w = Vec::with_capacity(side * side * 4);
        for y in 0..side {
            for x in 0..side {
                w.extend_from_slice(&f(x, y));
            }
        }
        w
    }

    fn at(w: &[f32], side: usize, x: usize, y: usize, c: usize) -> f32 {
        w.get((y * side + x) * 4 + c).copied().unwrap_or(f32::NAN)
    }

    #[test]
    fn impulse_response_is_symmetric_and_normalised() {
        for sigma in [1.0_f32, 3.0, 12.0] {
            let Some(spec) = GaussianSpec::new(sigma) else {
                unreachable!("spec");
            };
            let side = 2 * (spec.reach() as usize + 4) + 1;
            let c = side / 2;
            let mut w = window(side, |x, y| {
                if (x, y) == (c, c) {
                    [1.0, 0.0, 0.0, 1.0]
                } else {
                    [0.0; 4]
                }
            });
            gaussian_blur_window(&mut w, side, &spec);
            let mut total = 0.0_f64;
            for y in 0..side {
                for x in 0..side {
                    let v = at(&w, side, x, y, 0);
                    total += f64::from(v);
                    // Mirror symmetric in x, y and the diagonal.
                    assert_eq!(v, at(&w, side, side - 1 - x, y, 0), "x mirror {sigma}");
                    assert_eq!(v, at(&w, side, x, side - 1 - y, 0), "y mirror {sigma}");
                    // Rows and columns round in a different order for boxes.
                    assert!(
                        (v - at(&w, side, y, x, 0)).abs() < 1e-5,
                        "transpose {sigma}"
                    );
                }
            }
            assert!((total - 1.0).abs() < 2e-3, "sigma {sigma}: total {total}");
            // The peak is where the impulse was, and decays outward.
            assert!(at(&w, side, c, c, 0) > at(&w, side, c + 1, c, 0));
        }
    }

    #[test]
    fn flat_area_stays_exactly_flat() {
        for sigma in [0.7_f32, 5.0, 40.0] {
            let Some(spec) = GaussianSpec::new(sigma) else {
                unreachable!("spec");
            };
            let side = 2 * spec.reach() as usize + 9;
            let mut w = window(side, |_, _| [0.25, 0.5, 0.125, 1.0]);
            gaussian_blur_window(&mut w, side, &spec);
            let m = spec.reach() as usize;
            for y in m..side - m {
                for x in m..side - m {
                    for (c, want) in [0.25_f32, 0.5, 0.125, 1.0].into_iter().enumerate() {
                        assert_eq!(at(&w, side, x, y, c), want, "sigma {sigma} at {x},{y}");
                    }
                }
            }
        }
    }

    /// A window computed alone equals the same pixels computed inside a
    /// bigger window, bit for bit (the seamlessness the tiles rely on).
    #[test]
    fn result_does_not_depend_on_the_window() {
        for sigma in [2.0_f32, 9.0] {
            let Some(spec) = GaussianSpec::new(sigma) else {
                unreachable!("spec");
            };
            let reach = spec.reach() as usize;
            let big = 64 + 2 * reach;
            let content = |x: usize, y: usize| -> [f32; 4] {
                let h = (x * 31 + y * 17 + (x * y) % 13) % 23;
                let a = (h as f32) / 22.0;
                [a * 0.5, a * 0.25, a * 0.75, a]
            };
            let mut whole = window(big, content);
            gaussian_blur_window(&mut whole, big, &spec);
            // A smaller window around the central 16 x 16.
            let small = 16 + 2 * reach;
            let off = (big - small) / 2;
            let mut part = window(small, |x, y| content(x + off, y + off));
            gaussian_blur_window(&mut part, small, &spec);
            for y in 0..16 {
                for x in 0..16 {
                    for c in 0..4 {
                        assert_eq!(
                            at(&part, small, x + reach, y + reach, c).to_bits(),
                            at(&whole, big, x + off + reach, y + off + reach, c).to_bits(),
                            "sigma {sigma} ({x},{y}) c{c}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn exact_kernel_matches_a_float_reference_for_one_axis() {
        let sigma = 2.0_f64;
        let Some(spec) = GaussianSpec::new(2.0) else {
            unreachable!("spec");
        };
        let side = 41;
        let c = 20;
        let mut w = window(
            side,
            |x, y| {
                if y == c && x == c { [1.0; 4] } else { [0.0; 4] }
            },
        );
        gaussian_blur_window(&mut w, side, &spec);
        // Separable: the response at (c+dx, c+dy) is g(dx) g(dy).
        let g = |d: f64| (-(d * d) / (2.0 * sigma * sigma)).exp();
        let norm: f64 = (-10_i32..=10).map(|d| g(f64::from(d))).sum();
        for dx in -3_i32..=3 {
            for dy in -3_i32..=3 {
                let want = g(f64::from(dx)) * g(f64::from(dy)) / (norm * norm);
                let got = f64::from(at(
                    &w,
                    side,
                    (c as i32 + dx) as usize,
                    (c as i32 + dy) as usize,
                    0,
                ));
                assert!((got - want).abs() < 2e-4, "{dx},{dy}: {got} vs {want}");
            }
        }
    }

    /// Against an independent f64 convolution of the three boxes, and
    /// against the true Gaussian within 6 % of its peak.
    #[test]
    fn three_boxes_match_their_reference_and_approximate_a_gaussian() {
        let Some(spec) = GaussianSpec::new(20.0) else {
            unreachable!("spec");
        };
        let GaussianSpec::Boxes { radii } = &spec else {
            unreachable!("boxes");
        };
        let mut kernel = vec![1.0_f64];
        for r in radii {
            let w = 2 * *r as usize + 1;
            let mut next = vec![0.0_f64; kernel.len() + w - 1];
            for (i, k) in kernel.iter().enumerate() {
                for j in 0..w {
                    if let Some(slot) = next.get_mut(i + j) {
                        *slot += k / w as f64;
                    }
                }
            }
            kernel = next;
        }
        let half = kernel.len() / 2;
        let side = 2 * spec.reach() as usize + 21;
        let c = side / 2;
        let mut w = window(
            side,
            |x, y| if (x, y) == (c, c) { [1.0; 4] } else { [0.0; 4] },
        );
        gaussian_blur_window(&mut w, side, &spec);
        let sigma = 20.0_f64;
        let gauss_peak = 1.0 / (sigma * (2.0 * std::f64::consts::PI).sqrt());
        for dx in [-45_i32, -20, -1, 0, 3, 17, 39] {
            for dy in [-30_i32, 0, 8, 41] {
                let kx = kernel
                    .get((half as i32 + dx) as usize)
                    .copied()
                    .unwrap_or(f64::NAN);
                let ky = kernel
                    .get((half as i32 + dy) as usize)
                    .copied()
                    .unwrap_or(f64::NAN);
                let got = f64::from(at(
                    &w,
                    side,
                    (c as i32 + dx) as usize,
                    (c as i32 + dy) as usize,
                    0,
                ));
                assert!(
                    (got - kx * ky).abs() < 2e-6,
                    "{dx},{dy}: {got} vs {}",
                    kx * ky
                );
                let g = |d: i32| {
                    (-(f64::from(d) * f64::from(d)) / (2.0 * sigma * sigma)).exp() * gauss_peak
                };
                assert!(
                    (kx - g(dx)).abs() < 0.06 * gauss_peak,
                    "1-D vs Gaussian at {dx}"
                );
            }
        }
    }

    #[test]
    fn non_finite_and_bad_sizes_are_safe() {
        let Some(spec) = GaussianSpec::new(1.0) else {
            unreachable!("spec");
        };
        let mut w = window(9, |_, _| [f32::NAN, f32::INFINITY, 0.0, 1.0]);
        gaussian_blur_window(&mut w, 9, &spec);
        assert!(w.iter().all(|v| v.is_finite()));
        let mut short = vec![0.5_f32; 10];
        gaussian_blur_window(&mut short, 9, &spec);
        assert_eq!(short, vec![0.5_f32; 10]);
        assert!(crop_window(&w, 9, 4).is_empty()); // odd margin
        assert_eq!(crop_window(&w, 9, 5).len(), 5 * 5 * 4);
    }

    /// Timing only: one tile's blur at several radii.
    /// `cargo test -p aurora-filters --lib time_a_tile_blur -- --ignored --nocapture`
    #[test]
    #[ignore = "timing only"]
    fn time_a_tile_blur() {
        for sigma in [2.0_f32, 4.0, 5.0, 10.0, 20.0, 40.0, 85.0] {
            let Some(spec) = GaussianSpec::new(sigma) else {
                unreachable!("spec");
            };
            let side = 256 + 2 * spec.reach() as usize;
            let mut best = std::time::Duration::MAX;
            for _ in 0..3 {
                let mut w = window(side, |x, y| {
                    let a = ((x * 7 + y * 13) % 17) as f32 / 16.0;
                    [a * 0.5, a * 0.25, a * 0.75, a]
                });
                let started = std::time::Instant::now();
                gaussian_blur_window(&mut w, side, &spec);
                best = best.min(started.elapsed());
            }
            println!("sigma {sigma}: window {side}^2 {best:?}");
        }
    }
}
