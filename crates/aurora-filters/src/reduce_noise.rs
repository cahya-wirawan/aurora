//! Reduce Noise of a premultiplied RGBA window (task 178).
//!
//! **This is Aurora's own model, not Photoshop's algorithm** (which is not
//! published). Photoshop's four controls are kept; what they do here is a
//! choice, compared with nothing. Three stages run in this order, each on
//! the result of the one before, and a stage whose setting is `0` is
//! skipped outright, so all four settings at `0` leave every bit alone.
//!
//! 1. **Strength** (0..=10) is a bilateral filter on *straight* colour,
//!    all three channels together: a pixel becomes the mean of its
//!    neighbours within `radius = ceil(strength / 2)` pixels (1..=5, a
//!    square window), each weighted by
//!    `spatial * range * neighbour alpha`. `spatial` is a Gaussian of
//!    standard deviation `max(0.6, strength / 4)` px; `range` is
//!    `exp(-d^2 / (2 s^2))` with `d^2` the squared distance of the two
//!    straight colours in RGB and
//!    `s = 0.02 + 0.28 * (1 - preserve_details / 100)`: at Preserve
//!    Details 100 % only neighbours within about 5 levels count, so edges
//!    and texture survive and only fine grain is averaged; at 0 % the
//!    range term is wide (`s = 0.3`) and the filter approaches a plain
//!    blur. Weighting by the neighbour's alpha keeps transparent pixels
//!    (whose stored colour means nothing) from bleeding in. The exponential
//!    is read from a 2,048-entry table over `d^2 <= 16 s^2` (zero beyond).
//! 2. **Reduce Color Noise** (0..=100 %) blurs the chroma, `colour - Y`
//!    with `Y` the Rec.709 luma, with the exact Gaussian of
//!    [`crate::blur`] at `sigma = color_noise / 100 * 4` px (alpha-weighted
//!    like the unsharp mask's blur), and adds it back to the unchanged
//!    luma: grain that is only in the colour goes, brightness detail stays
//!    (up to a clamp to `[min(0, c), max(1, c)]` per channel).
//! 3. **Sharpen Details** (0..=100 %) is [`crate::unsharp_mask_window`]
//!    with amount = the percentage, radius 1 px, threshold 0.
//!
//! Alpha is never changed; a pixel with no alpha, or a non-finite sample,
//! is left as stored.
//!
//! **Reach.** The three stages read one after the other, so the total
//! reach is the sum of the stage reaches: `radius` +
//! `chroma spec reach` + `sharpen spec reach` (each 0 for a stage that is
//! off): [`aurora_core::FilterKind::reach`] computes it from the same
//! functions this module uses. A pixel's result depends only on window
//! content within that distance, and every sum runs in a fixed order, so
//! tiles drawn from different windows agree bit for bit wherever each is
//! at least that far from its window edge. At most 5 + 12 + 3 = 20 px.

use aurora_core::{
    GaussianSpec, MAX_NOISE_PERCENT, MAX_NOISE_STRENGTH, reduce_noise_chroma_spec,
    reduce_noise_radius, reduce_noise_sharpen_spec,
};

use crate::blur::gaussian_blur_window;
use crate::unsharp::unsharp_mask_window;

const CHANNELS: usize = 4;
const RANGE_TABLE: usize = 2048;
/// The range term is cut off at `d^2 = RANGE_CUTOFF * s^2` (`exp(-8)`).
const RANGE_CUTOFF: f32 = 16.0;
/// Rec.709 luma weights (the Dodge/Burn/Sponge tools use the same).
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Runs Reduce Noise over a `side` x `side` window of premultiplied RGBA
/// `f32` in place. Settings outside their ranges are clamped; a
/// non-finite one counts as 0. A window whose length is not
/// `side * side * 4` changes nothing.
///
/// The outer ring of the window, as wide as the total reach, is computed
/// from a truncated neighbourhood and is not exact; callers crop it away.
pub fn reduce_noise_window(
    window: &mut [f32],
    side: usize,
    strength: f32,
    preserve_details: f32,
    color_noise: f32,
    sharpen_details: f32,
) {
    let Some(len) = side.checked_mul(side).and_then(|n| n.checked_mul(CHANNELS)) else {
        return;
    };
    if len != window.len() {
        return;
    }
    let clean = |v: f32, max: f32| {
        if v.is_finite() {
            v.clamp(0.0, max)
        } else {
            0.0
        }
    };
    let strength = clean(strength, MAX_NOISE_STRENGTH);
    let preserve = clean(preserve_details, MAX_NOISE_PERCENT);
    let colour = clean(color_noise, MAX_NOISE_PERCENT);
    let sharpen = clean(sharpen_details, MAX_NOISE_PERCENT);

    let radius = reduce_noise_radius(strength);
    if radius > 0 {
        bilateral(window, side, radius, strength, preserve);
    }
    if let Some(spec) = reduce_noise_chroma_spec(colour) {
        blur_chroma(window, side, &spec);
    }
    if let Some(spec) = reduce_noise_sharpen_spec(sharpen) {
        unsharp_mask_window(window, side, sharpen, 0, &spec);
    }
}

/// A pixel's straight colour when it can take part (alpha above 0, all
/// samples finite).
fn straight(texel: &[f32]) -> Option<([f32; 3], f32)> {
    let a = *texel.get(3)?;
    let rgb = [*texel.first()?, *texel.get(1)?, *texel.get(2)?];
    if !(a.is_finite() && a > 0.0 && rgb.iter().all(|v| v.is_finite())) {
        return None;
    }
    let c = rgb.map(|v| v / a);
    c.iter().all(|v| v.is_finite()).then_some((c, a))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn bilateral(window: &mut [f32], side: usize, radius: u32, strength: f32, preserve: f32) {
    let r = radius as usize;
    let width = 2 * r + 1;
    let sigma_s = (strength / 4.0).max(0.6);
    let sigma_r = 0.02 + 0.28 * (1.0 - preserve / MAX_NOISE_PERCENT);
    // Spatial weights, row-major over (dy, dx).
    let mut spatial = Vec::with_capacity(width * width);
    for dy in 0..width {
        for dx in 0..width {
            let ox = dx as f64 - r as f64;
            let oy = dy as f64 - r as f64;
            let w = (-(ox * ox + oy * oy) / (2.0 * f64::from(sigma_s) * f64::from(sigma_s))).exp();
            spatial.push(w as f32);
        }
    }
    let cutoff = RANGE_CUTOFF * sigma_r * sigma_r;
    let scale = RANGE_TABLE as f32 / cutoff;
    let table: Vec<f32> = (0..RANGE_TABLE)
        .map(|i| {
            let d2 = (i as f64 + 0.5) / f64::from(scale);
            (-d2 / (2.0 * f64::from(sigma_r) * f64::from(sigma_r))).exp() as f32
        })
        .collect();
    // Straight colour and alpha, read once; `None` for a pixel that does
    // not take part.
    let source: Vec<Option<([f32; 3], f32)>> =
        window.chunks_exact(CHANNELS).map(straight).collect();
    for y in 0..side {
        for x in 0..side {
            let Some(Some((centre, alpha))) = source.get(y * side + x) else {
                continue;
            };
            let mut sum_w = 0.0_f32;
            let mut sum = [0.0_f32; 3];
            for dy in 0..width {
                let Some(ny) = (y + dy).checked_sub(r).filter(|&v| v < side) else {
                    continue;
                };
                for dx in 0..width {
                    let Some(nx) = (x + dx).checked_sub(r).filter(|&v| v < side) else {
                        continue;
                    };
                    let (Some(Some((c, a))), Some(&sw)) =
                        (source.get(ny * side + nx), spatial.get(dy * width + dx))
                    else {
                        continue;
                    };
                    let d2: f32 = c.iter().zip(centre).map(|(q, p)| (q - p) * (q - p)).sum();
                    let index = (d2 * scale) as usize;
                    let Some(&rw) = table.get(index) else {
                        continue;
                    };
                    let w = sw * rw * a;
                    sum_w += w;
                    for (acc, q) in sum.iter_mut().zip(c) {
                        *acc += w * q;
                    }
                }
            }
            if !(sum_w.is_finite() && sum_w > 0.0) {
                continue;
            }
            let base = (y * side + x) * CHANNELS;
            for (k, acc) in sum.iter().enumerate() {
                let value = acc / sum_w;
                if value.is_finite()
                    && let Some(slot) = window.get_mut(base + k)
                {
                    *slot = value * alpha;
                }
            }
        }
    }
}

fn luma(c: &[f32; 3]) -> f32 {
    c.iter().zip(LUMA).map(|(v, w)| v * w).sum()
}

fn blur_chroma(window: &mut [f32], side: usize, spec: &GaussianSpec) {
    let mut chroma = vec![0.0_f32; window.len()];
    for (texel, out) in window
        .chunks_exact(CHANNELS)
        .zip(chroma.chunks_exact_mut(CHANNELS))
    {
        if let Some((c, a)) = straight(texel) {
            let y = luma(&c);
            for (slot, v) in out.iter_mut().zip(c) {
                *slot = (v - y) * a;
            }
            if let Some(slot) = out.get_mut(3) {
                *slot = a;
            }
        }
    }
    gaussian_blur_window(&mut chroma, side, spec);
    for (texel, blurred) in window
        .chunks_exact_mut(CHANNELS)
        .zip(chroma.chunks_exact(CHANNELS))
    {
        let Some((c, a)) = straight(texel) else {
            continue;
        };
        let Some(&blur_a) = blurred.get(3) else {
            continue;
        };
        if !(blur_a.is_finite() && blur_a > 0.0) {
            continue;
        }
        let y = luma(&c);
        for ((slot, &own), &blur_c) in texel.iter_mut().zip(&c).zip(blurred) {
            let value = (y + blur_c / blur_a).clamp(own.min(0.0), own.max(1.0));
            if value.is_finite() {
                *slot = value * a;
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use super::*;

    /// A deterministic pseudo-random value in `[-1, 1)` for a seed.
    fn noise(seed: u64) -> f32 {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        ((z >> 40) as f32 / (1_u64 << 23) as f32) - 1.0
    }

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

    /// Opaque grey grain of amplitude `amp` around `base`.
    fn grainy(side: usize, amp: f32, base: f32) -> Vec<f32> {
        window(side, |x, y| {
            let v = base + amp * noise((y * side + x) as u64);
            [v, v, v, 1.0]
        })
    }

    fn std_dev(w: &[f32], side: usize, lo: usize, hi: usize) -> f32 {
        let mut vals = Vec::new();
        for y in lo..hi {
            for x in lo..hi {
                vals.push(at(w, side, x, y, 0));
            }
        }
        let mean = vals.iter().sum::<f32>() / vals.len() as f32;
        (vals.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / vals.len() as f32).sqrt()
    }

    fn total_reach(strength: f32, colour: f32, sharpen: f32) -> usize {
        aurora_core::FilterKind::ReduceNoise {
            strength,
            preserve_details: 50.0,
            color_noise: colour,
            sharpen_details: sharpen,
        }
        .reach() as usize
    }

    #[test]
    fn all_zero_is_the_identity_bit_for_bit() {
        let side = 33;
        let mut w = window(side, |x, y| {
            let a = 0.25 + 0.75 * noise((x * 7 + y * 131) as u64).abs();
            [
                noise((x + y * side) as u64) * a,
                0.3 * a,
                noise((x * 3 + y) as u64) * 0.5,
                if x % 5 == 0 { 0.0 } else { a },
            ]
        });
        let before = w.clone();
        reduce_noise_window(&mut w, side, 0.0, 70.0, 0.0, 0.0);
        let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&w), bits(&before));
        // Non-finite or negative settings count as off, too.
        reduce_noise_window(&mut w, side, f32::NAN, 0.0, -3.0, f32::NAN);
        assert_eq!(bits(&w), bits(&before));
        // A wrong length changes nothing.
        let mut short = vec![0.5_f32; 10];
        reduce_noise_window(&mut short, side, 10.0, 0.0, 100.0, 100.0);
        assert_eq!(short, vec![0.5_f32; 10]);
    }

    #[test]
    fn strength_smooths_grain_and_more_strength_smooths_more() {
        let side = 41;
        let base = grainy(side, 0.08, 0.5);
        let sd = |strength| {
            let mut w = base.clone();
            reduce_noise_window(&mut w, side, strength, 0.0, 0.0, 0.0);
            std_dev(&w, side, 8, 33)
        };
        let raw = std_dev(&base, side, 8, 33);
        let (a, b) = (sd(2.0), sd(10.0));
        assert!(a < raw * 0.8, "{a} vs {raw}");
        assert!(b < a, "{b} vs {a}");
        // Alpha untouched.
        let mut w = base.clone();
        reduce_noise_window(&mut w, side, 10.0, 0.0, 0.0, 0.0);
        for y in 0..side {
            for x in 0..side {
                assert_eq!(at(&w, side, x, y, 3), 1.0);
            }
        }
    }

    #[test]
    fn preserve_details_keeps_an_edge_that_a_low_setting_softens() {
        let side = 41;
        let make = || {
            window(side, |x, y| {
                let v = if x < 20 { 0.4 } else { 0.6 } + 0.002 * noise((y * side + x) as u64);
                [v, v, v, 1.0]
            })
        };
        let step = |preserve| {
            let mut w = make();
            reduce_noise_window(&mut w, side, 10.0, preserve, 0.0, 0.0);
            at(&w, side, 20, 20, 0) - at(&w, side, 19, 20, 0)
        };
        let (low, high) = (step(0.0), step(100.0));
        assert!(high > 0.19, "edge kept at 100 %: {high}");
        assert!(low < high - 0.05, "0 % softens it: {low} vs {high}");
    }

    #[test]
    fn colour_noise_blurs_chroma_and_keeps_luma() {
        let side = 41;
        // Equal-luma speckle: red and cyan-ish pairs chosen per pixel.
        let w0 = window(side, |x, y| {
            let t = noise((y * side + x) as u64) * 0.1;
            [0.5 + t, 0.5, 0.5 - t * 0.2126 / 0.0722, 1.0]
        });
        let luma_of = |w: &[f32], x, y| {
            LUMA[0] * at(w, side, x, y, 0)
                + LUMA[1] * at(w, side, x, y, 1)
                + LUMA[2] * at(w, side, x, y, 2)
        };
        let mut w = w0.clone();
        reduce_noise_window(&mut w, side, 0.0, 0.0, 100.0, 0.0);
        let chroma_sd = |w: &[f32]| {
            let mut vals = Vec::new();
            for y in 10..31 {
                for x in 10..31 {
                    vals.push(at(w, side, x, y, 0) - luma_of(w, x, y));
                }
            }
            let mean = vals.iter().sum::<f32>() / vals.len() as f32;
            (vals.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / vals.len() as f32).sqrt()
        };
        assert!(chroma_sd(&w) < chroma_sd(&w0) * 0.3);
        for (x, y) in [(15, 15), (20, 22), (30, 12)] {
            assert!(
                (luma_of(&w, x, y) - luma_of(&w0, x, y)).abs() < 1e-4,
                "{x},{y}"
            );
        }
        // A bigger percentage smooths more.
        let mut mid = w0.clone();
        reduce_noise_window(&mut mid, side, 0.0, 0.0, 30.0, 0.0);
        assert!(chroma_sd(&w) < chroma_sd(&mid) && chroma_sd(&mid) < chroma_sd(&w0));
    }

    #[test]
    fn sharpen_details_is_the_unsharp_mask_at_radius_one() {
        let side = 41;
        let make = || {
            window(side, |x, _| {
                let v = if x < 20 { 0.3 } else { 0.7 };
                [v, v, v, 1.0]
            })
        };
        let mut a = make();
        reduce_noise_window(&mut a, side, 0.0, 0.0, 0.0, 60.0);
        let mut b = make();
        let spec = GaussianSpec::new(1.0).unwrap_or_else(|| unreachable!());
        unsharp_mask_window(&mut b, side, 60.0, 0, &spec);
        assert_eq!(a, b);
        assert!(at(&a, side, 19, 20, 0) < 0.3, "undershoot");
    }

    #[test]
    fn transparent_pixels_stay_and_do_not_bleed_colour() {
        let side = 21;
        // Left half transparent (premultiplied zeros), right half red at
        // half alpha with grain; a fully transparent column stays as is.
        let mut w = window(side, |x, y| {
            if x < 10 {
                [0.0, 0.0, 0.0, 0.0]
            } else {
                let a = 0.5;
                let g = 0.05 * noise((y * side + x) as u64);
                [(0.8 + g) * a, 0.1 * a, 0.1 * a, a]
            }
        });
        let before = w.clone();
        reduce_noise_window(&mut w, side, 10.0, 0.0, 100.0, 50.0);
        for y in 0..side {
            for x in 0..side {
                assert_eq!(at(&w, side, x, y, 3), at(&before, side, x, y, 3));
                if x < 10 {
                    for c in 0..4 {
                        assert_eq!(at(&w, side, x, y, c).to_bits(), 0.0_f32.to_bits());
                    }
                }
            }
        }
        // The straight colour next to the transparent edge is still red:
        // transparent neighbours did not darken it.
        let a = at(&w, side, 10, 10, 3);
        let r = at(&w, side, 10, 10, 0) / a;
        assert!(r > 0.7, "straight red {r}");
    }

    /// The result at a pixel depends only on input within the total reach,
    /// and a differently cut window gives identical bits there.
    #[test]
    fn reach_is_exact_and_windows_agree_bit_for_bit() {
        let big = 96_usize;
        let source = window(big, |x, y| {
            let a = if (x / 7 + y / 5) % 6 == 0 { 0.4 } else { 1.0 };
            let v = 0.5 + 0.3 * noise((y * big + x) as u64);
            [
                v * a,
                (0.4 + 0.2 * noise((x * big + y + 1) as u64)) * a,
                0.3 * a,
                a,
            ]
        });
        let cut = |ox: usize, oy: usize, side: usize| {
            let mut w = Vec::with_capacity(side * side * 4);
            for y in 0..side {
                for x in 0..side {
                    let i = ((oy + y) * big + ox + x) * 4;
                    w.extend_from_slice(source.get(i..i + 4).unwrap_or(&[0.0; 4]));
                }
            }
            w
        };
        for (strength, preserve, colour, sharpen) in [
            (10.0_f32, 30.0_f32, 100.0_f32, 100.0_f32),
            (3.0, 80.0, 40.0, 0.0),
            (0.0, 0.0, 70.0, 25.0),
            (7.5, 100.0, 0.0, 0.0),
        ] {
            let reach = total_reach(strength, colour, sharpen);
            let run = |ox, oy, side| {
                let mut w = cut(ox, oy, side);
                reduce_noise_window(&mut w, side, strength, preserve, colour, sharpen);
                w
            };
            // Window A at (8, 8) 64 wide, window B at (20, 14) 60 wide.
            let (a, b) = (run(8, 8, 64), run(20, 14, 60));
            let mut compared = 0;
            for gy in 20..70_usize {
                for gx in 28..68_usize {
                    let in_a = |g: usize, o: usize, s: usize| g >= o + reach && g < o + s - reach;
                    if !(in_a(gx, 8, 64) && in_a(gy, 8, 64) && in_a(gx, 20, 60) && in_a(gy, 14, 60))
                    {
                        continue;
                    }
                    for c in 0..4 {
                        assert_eq!(
                            at(&a, 64, gx - 8, gy - 8, c).to_bits(),
                            at(&b, 60, gx - 20, gy - 14, c).to_bits(),
                            "{strength} {colour} {sharpen} at {gx},{gy} c{c}"
                        );
                    }
                    compared += 1;
                }
            }
            assert!(compared > 100, "compared {compared}");
            // An edit just beyond the reach of a pixel leaves it alone; one
            // inside the reach changes it (so the bound is tight enough to
            // matter) when the stage reach is non-zero.
            let (px, py) = (32_usize, 32_usize);
            let base = run(0, 0, 65);
            let poke = |dx: usize| {
                let mut w = cut(0, 0, 65);
                // A neighbour whose straight colour is the centre's plus
                // a little, so the range term lets it count.
                let own = (py * 65 + px) * 4;
                let at_n = (py * 65 + px + dx) * 4;
                let a_n = w.get(at_n + 3).copied().unwrap_or(1.0);
                let a_c = w.get(own + 3).copied().unwrap_or(1.0);
                for k in 0..3 {
                    let c = w.get(own + k).copied().unwrap_or(0.0) / a_c + 0.01;
                    if let Some(slot) = w.get_mut(at_n + k) {
                        *slot = c * a_n;
                    }
                }
                reduce_noise_window(&mut w, 65, strength, preserve, colour, sharpen);
                w
            };
            let outside = poke(reach + 1);
            for c in 0..4 {
                assert_eq!(
                    at(&outside, 65, px, py, c).to_bits(),
                    at(&base, 65, px, py, c).to_bits()
                );
            }
            if reach > 0 {
                let inside = poke(1);
                assert_ne!(
                    (0..3)
                        .map(|c| at(&inside, 65, px, py, c).to_bits())
                        .collect::<Vec<_>>(),
                    (0..3)
                        .map(|c| at(&base, 65, px, py, c).to_bits())
                        .collect::<Vec<_>>(),
                    "{strength} {colour} {sharpen}"
                );
            }
        }
    }

    /// Cost of one 256 x 256 tile at the maximum settings, printed (run
    /// with `--ignored --nocapture`).
    #[test]
    #[ignore = "timing"]
    fn timing_of_one_tile_at_maximum_settings() {
        let reach = total_reach(10.0, 100.0, 100.0);
        let side = 256 + 2 * reach;
        let base = grainy(side, 0.1, 0.5);
        let mut times = Vec::new();
        for _ in 0..5 {
            let mut w = base.clone();
            let start = std::time::Instant::now();
            reduce_noise_window(&mut w, side, 10.0, 0.0, 100.0, 100.0);
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        println!("reach {reach}, window {side}, ms per tile: {times:?}");
        for (name, s) in [
            ("strength 10 only", [10.0, 0.0, 0.0, 0.0]),
            ("colour 100 only", [0.0, 0.0, 100.0, 0.0]),
            ("sharpen 100 only", [0.0, 0.0, 0.0, 100.0]),
        ] {
            let mut w = base.clone();
            let start = std::time::Instant::now();
            reduce_noise_window(&mut w, side, s[0], s[1], s[2], s[3]);
            println!("{name}: {:.1} ms", start.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
