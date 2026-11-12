//! Unsharp Mask of a premultiplied RGBA window (task 177).
//!
//! `out = orig + amount * (orig - blur(orig))`, where the blur is the
//! exact [`crate::blur`] Gaussian, so a pixel's result depends only on the
//! source values within the blur's reach, never on where the window starts:
//! tiles are seamless bit for bit, as for the blur itself.
//!
//! **Colour space.** The sharpening acts on *straight* colour, the colour
//! a pixel has when it is drawn, not on premultiplied values: a premultiplied
//! difference would also "sharpen" alpha edges into dark or bright rims.
//! The blurred straight colour is the blurred premultiplied colour over the
//! blurred alpha, which weights each neighbour by its opacity, so
//! transparent neighbours (whose stored colour means nothing) do not pull
//! the local mean toward black. Alpha is never changed (Photoshop sharpens
//! colour only), a pixel with no alpha stays transparent, and colour is
//! clamped to `[min(0, orig), max(1, orig)]`: sharpening can neither push a
//! colour outside the displayable range nor darken or brighten an
//! extended-range (HDR) pixel the filter did not change.
//!
//! **Threshold.** Per channel: a channel whose straight colour differs from
//! the blurred one by less than `threshold` 8-bit levels (`|diff| * 255 <
//! threshold`) is left exactly as stored; at threshold 0 every channel is
//! sharpened. Photoshop's own rule is not documented in detail; a
//! per-channel difference is the closest reading of "levels" and keeps each
//! channel independent, so a colour fringe is not created by one channel
//! being sharpened and the others not (a luminosity threshold was
//! considered and rejected for that reason).

use aurora_core::GaussianSpec;

use crate::blur::gaussian_blur_window;

const CHANNELS: usize = 4;

/// Sharpens a `side` x `side` window of premultiplied RGBA `f32` in place.
/// `amount` is a percentage (150 adds back 1.5 times the difference),
/// `threshold` is in 8-bit levels. A non-finite `amount` or a window whose
/// length is not `side * side * 4` changes nothing.
///
/// The outer ring of the window, as wide as the spec's reach, is computed
/// from a truncated neighbourhood and is not exact; callers crop it away.
pub fn unsharp_mask_window(
    window: &mut [f32],
    side: usize,
    amount: f32,
    threshold: u8,
    spec: &GaussianSpec,
) {
    let Some(len) = side.checked_mul(side).and_then(|n| n.checked_mul(CHANNELS)) else {
        return;
    };
    if len != window.len() || !amount.is_finite() {
        return;
    }
    let mut blurred = window.to_vec();
    gaussian_blur_window(&mut blurred, side, spec);
    let gain = amount / 100.0;
    let limit = f32::from(threshold);
    for (texel, blur) in window
        .chunks_exact_mut(CHANNELS)
        .zip(blurred.chunks_exact(CHANNELS))
    {
        let (Some(&a), Some(&blur_a)) = (texel.get(3), blur.get(3)) else {
            continue;
        };
        if !(a.is_finite() && a > 0.0 && blur_a.is_finite() && blur_a > 0.0) {
            continue;
        }
        for (slot, &blur_c) in texel.iter_mut().zip(blur).take(3) {
            let stored = *slot;
            if !stored.is_finite() {
                continue;
            }
            let straight = stored / a;
            let diff = straight - blur_c / blur_a;
            if !diff.is_finite() || diff == 0.0 || diff.abs() * 255.0 < limit {
                continue;
            }
            let sharpened = (straight + gain * diff).clamp(straight.min(0.0), straight.max(1.0));
            // A difference of exactly zero kept the stored bits above (a
            // flat area stays as it was, not re-derived through a division
            // and a multiplication); a tiny one (Q24 rounding in the blur)
            // moves the value by far less than an `f16` step.
            if sharpened.is_finite() {
                *slot = sharpened * a;
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
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

    fn spec(sigma: f32) -> GaussianSpec {
        GaussianSpec::new(sigma).unwrap_or_else(|| unreachable!("sigma {sigma}"))
    }

    /// An opaque vertical edge: grey 0.25 left of `edge`, 0.75 from it.
    fn edge(side: usize, edge_x: usize) -> Vec<f32> {
        window(side, |x, _| {
            let v = if x < edge_x { 0.25 } else { 0.75 };
            [v, v, v, 1.0]
        })
    }

    #[test]
    fn flat_area_is_unchanged() {
        let side = 41;
        let mut w = window(side, |_, _| [0.25, 0.5, 0.125, 1.0]);
        let before = w.clone();
        unsharp_mask_window(&mut w, side, 500.0, 0, &spec(2.0));
        let m = spec(2.0).reach() as usize;
        for y in m..side - m {
            for x in m..side - m {
                for c in 0..4 {
                    assert_eq!(
                        at(&w, side, x, y, c),
                        at(&before, side, x, y, c),
                        "{x},{y} c{c}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_edge_overshoots_by_the_amount_times_the_difference() {
        let side = 41;
        let sigma = 2.0;
        let s = spec(sigma);
        let mut w = edge(side, 20);
        let blurred = {
            let mut b = w.clone();
            gaussian_blur_window(&mut b, side, &s);
            b
        };
        unsharp_mask_window(&mut w, side, 100.0, 0, &s);
        // Next to the edge: the dark side gets darker, the bright side
        // brighter, by exactly orig - blur (amount 100 %).
        for (x, orig) in [(19, 0.25_f32), (20, 0.75)] {
            let b = at(&blurred, side, x, 20, 0);
            let want = orig + (orig - b);
            assert!(
                (at(&w, side, x, 20, 0) - want).abs() < 1e-5,
                "x {x}: {} vs {want}",
                at(&w, side, x, 20, 0)
            );
        }
        assert!(at(&w, side, 19, 20, 0) < 0.25 - 0.02, "dark undershoot");
        assert!(at(&w, side, 20, 20, 0) > 0.75 - 1e-6, "bright side");
        // Far from the edge nothing moved.
        assert_eq!(at(&w, side, 5, 20, 0), 0.25);
        assert_eq!(at(&w, side, 35, 20, 0), 0.75);
        // Alpha untouched everywhere.
        for y in 0..side {
            for x in 0..side {
                assert_eq!(at(&w, side, x, y, 3), 1.0);
            }
        }
    }

    #[test]
    fn amount_scales_the_added_difference_linearly() {
        let side = 41;
        let s = spec(2.0);
        // A mild edge, so 200 % does not reach the clamp.
        let run = |amount| {
            let mut w = window(side, |x, _| {
                let v = if x < 20 { 0.45 } else { 0.55 };
                [v, v, v, 1.0]
            });
            unsharp_mask_window(&mut w, side, amount, 0, &s);
            at(&w, side, 19, 20, 0)
        };
        let one = 0.45 - run(100.0);
        let two = 0.45 - run(200.0);
        let half = 0.45 - run(50.0);
        assert!(one > 0.0);
        assert!((two - 2.0 * one).abs() < 1e-5, "{two} vs {one}");
        assert!((half - 0.5 * one).abs() < 1e-5, "{half} vs {one}");
    }

    #[test]
    fn threshold_leaves_small_differences_alone() {
        let side = 41;
        let s = spec(2.0);
        // A faint edge: 0.5 -> 0.5 + 6/255; the largest straight difference
        // from the blur is below 3 levels.
        let faint = |threshold| {
            let mut w = window(side, |x, _| {
                let v = if x < 20 { 0.5 } else { 0.5 + 6.0 / 255.0 };
                [v, v, v, 1.0]
            });
            let before = w.clone();
            unsharp_mask_window(&mut w, side, 300.0, threshold, &s);
            (before, w)
        };
        let (before, w) = faint(0);
        assert_ne!(before, w, "threshold 0 sharpens a faint edge");
        let (before, w) = faint(4);
        assert_eq!(before, w, "threshold 4 leaves a 3-level difference alone");
        // The strong edge still sharpens at the same threshold.
        let mut strong = edge(side, 20);
        let orig = strong.clone();
        unsharp_mask_window(&mut strong, side, 300.0, 4, &s);
        assert_ne!(strong, orig);
        // It is per channel: only the channel with the big difference moves.
        let mut mixed = window(side, |x, _| {
            let r = if x < 20 { 0.25 } else { 0.75 };
            [r, 0.5, 0.5, 1.0]
        });
        unsharp_mask_window(&mut mixed, side, 300.0, 4, &s);
        assert!(at(&mixed, side, 19, 20, 0) != 0.25);
        assert_eq!(at(&mixed, side, 19, 20, 1), 0.5);
        assert_eq!(at(&mixed, side, 19, 20, 2), 0.5);
    }

    #[test]
    fn threshold_boundary_is_in_levels() {
        // One isolated step of exactly 8 levels sits at the threshold: it
        // is sharpened at 8 (not smaller than the threshold) and not at 9
        // when the blur-relative difference is below 9 levels.
        let side = 41;
        let s = spec(1.0);
        let make = || {
            window(side, |x, _| {
                let v = if x < 20 { 0.5 } else { 0.5 + 8.0 / 255.0 };
                [v, v, v, 1.0]
            })
        };
        let diff_levels = {
            let mut b = make();
            gaussian_blur_window(&mut b, side, &s);
            (at(&make(), side, 20, 20, 0) - at(&b, side, 20, 20, 0)).abs() * 255.0
        };
        assert!(diff_levels > 0.5 && diff_levels < 8.0, "{diff_levels}");
        let sharpened = |t: u8| {
            let mut w = make();
            unsharp_mask_window(&mut w, side, 200.0, t, &s);
            w != make()
        };
        let below = diff_levels.floor() as u8;
        let above = diff_levels.ceil() as u8;
        assert!(sharpened(below), "threshold {below} <= {diff_levels}");
        assert!(!sharpened(above), "threshold {above} > {diff_levels}");
    }

    #[test]
    fn transparent_pixels_stay_transparent_and_alpha_never_changes() {
        let side = 41;
        // An opaque white square on transparent black-ish garbage colour.
        let content = |x: usize, y: usize| -> [f32; 4] {
            if (14..27).contains(&x) && (14..27).contains(&y) {
                [0.9, 0.5, 0.2, 1.0]
            } else {
                [0.0, 0.0, 0.0, 0.0]
            }
        };
        let mut w = window(side, content);
        unsharp_mask_window(&mut w, side, 500.0, 0, &spec(3.0));
        for y in 0..side {
            for x in 0..side {
                assert_eq!(at(&w, side, x, y, 3), content(x, y)[3], "alpha {x},{y}");
                if content(x, y)[3] == 0.0 {
                    for c in 0..3 {
                        assert_eq!(at(&w, side, x, y, c), 0.0, "{x},{y} c{c}");
                    }
                }
            }
        }
        // A semi-transparent soft pixel keeps alpha; its colour is straight
        // (no dark rim from the transparent neighbours): colour never goes
        // below zero or above one.
        let mut soft = window(side, |x, _| {
            if x < 20 {
                [0.0, 0.0, 0.0, 0.0]
            } else {
                [0.4, 0.4, 0.4, 0.5]
            }
        });
        unsharp_mask_window(&mut soft, side, 500.0, 0, &spec(2.0));
        for y in 0..side {
            for x in 20..side {
                let a = at(&soft, side, x, y, 3);
                assert_eq!(a, 0.5);
                let straight = at(&soft, side, x, y, 0) / a;
                assert!((0.0..=1.0).contains(&straight), "{straight}");
                // Away from the window's truncated ring the straight colour
                // of a uniform-colour region stays 0.8 (= 0.4 / 0.5).
                if (x, y) == (30, 20) {
                    assert!((straight - 0.8).abs() < 1e-5, "{straight}");
                }
            }
        }
    }

    #[test]
    fn colour_is_clamped_to_the_valid_range() {
        let side = 41;
        // A very hard black/white edge at 500 % overshoots far past 0 and 1.
        let mut w = window(side, |x, _| {
            let v = if x < 20 { 0.05 } else { 0.95 };
            [v, v, v, 1.0]
        });
        unsharp_mask_window(&mut w, side, 500.0, 0, &spec(2.0));
        let mut touched_low = false;
        let mut touched_high = false;
        for y in 0..side {
            for x in 0..side {
                let v = at(&w, side, x, y, 0);
                assert!((0.0..=1.0).contains(&v), "{v}");
                touched_low |= v == 0.0;
                touched_high |= v == 1.0;
            }
        }
        assert!(
            touched_low && touched_high,
            "the overshoot reached the clamps"
        );
        // An extended-range pixel the filter does not change is not clipped,
        // and one it does change is never pulled below its own value by the
        // upper clamp.
        let mut hdr = window(side, |_, _| [2.0, 2.0, 2.0, 1.0]);
        let before = hdr.clone();
        unsharp_mask_window(&mut hdr, side, 500.0, 0, &spec(2.0));
        assert_eq!(at(&hdr, side, 20, 20, 0), at(&before, side, 20, 20, 0));
    }

    #[test]
    fn hostile_input_never_produces_nan() {
        let side = 21;
        let mut w = window(side, |x, y| match (x + y) % 5 {
            0 => [f32::NAN, f32::INFINITY, -f32::INFINITY, 1.0],
            1 => [1.0, 1.0, 1.0, f32::NAN],
            2 => [0.5, 0.5, 0.5, 0.0],
            3 => [1e30, -1e30, 0.5, 1.0],
            _ => [0.2, 0.4, 0.6, 1e-30],
        });
        unsharp_mask_window(&mut w, side, 500.0, 0, &spec(1.5));
        // Colour of an opaque pixel stays finite; nothing is NaN except
        // what the input already was in a non-finite alpha (kept as is).
        for y in 0..side {
            for x in 0..side {
                for c in 0..3 {
                    let v = at(&w, side, x, y, c);
                    let a = at(&w, side, x, y, 3);
                    if a.is_finite() {
                        assert!(v.is_finite() || x + y == 0 || (x + y) % 5 == 0, "{v}");
                    }
                }
            }
        }
        // Wrong sizes and a non-finite amount change nothing.
        let mut short = vec![0.5_f32; 7];
        unsharp_mask_window(&mut short, 3, 100.0, 0, &spec(1.0));
        assert_eq!(short, vec![0.5_f32; 7]);
        let mut e = edge(11, 5);
        let before = e.clone();
        unsharp_mask_window(&mut e, 11, f32::NAN, 0, &spec(1.0));
        assert_eq!(e, before);
    }

    /// A window computed alone equals the same pixels computed inside a
    /// bigger window, bit for bit (the seamlessness tiles rely on), for the
    /// exact kernel and for the box approximation.
    #[test]
    fn result_does_not_depend_on_the_window() {
        for sigma in [2.0_f32, 9.0] {
            let s = spec(sigma);
            let reach = s.reach() as usize;
            let big = 64 + 2 * reach;
            let content = |x: usize, y: usize| -> [f32; 4] {
                let h = (x * 31 + y * 17 + (x * y) % 13) % 23;
                let a = 0.5 + (h as f32) / 44.0;
                [a * 0.5, a * 0.25, a * 0.75, a]
            };
            let mut whole = window(big, content);
            unsharp_mask_window(&mut whole, big, 180.0, 2, &s);
            let small = 16 + 2 * reach;
            let off = (big - small) / 2;
            let mut part = window(small, |x, y| content(x + off, y + off));
            unsharp_mask_window(&mut part, small, 180.0, 2, &s);
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
}
