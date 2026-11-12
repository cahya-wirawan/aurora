//! Dust & Scratches of a premultiplied RGBA window (task 179).
//!
//! **This is Aurora's model, close to Photoshop's description, not its
//! algorithm** (which is not published): a median filter that only
//! replaces a value that differs from the median by more than the
//! threshold.
//!
//! * The neighbourhood of a pixel is the **disc** of integer `radius`
//!   pixels around it, the pixel itself included: every offset
//!   `(dx, dy)` with `dx^2 + dy^2 <= radius^2` (radius 1 is the pixel and
//!   its four edge neighbours, 5 pixels; radius 2 is 13; radius 10 is
//!   317). Neighbours outside the window are not counted.
//! * The median is taken per channel on **straight** (un-premultiplied)
//!   colour, over the neighbours whose alpha is above 0 and whose samples
//!   are all finite (so transparent pixels, whose stored colour means
//!   nothing, never take part and nothing bleeds out of them, and a NaN
//!   is never chosen). With `n` such values sorted, the median is the
//!   value at index `(n - 1) / 2` (the **lower median** for an even `n`),
//!   so it is always one of the actual samples, never an average.
//! * A channel is replaced by its median only where
//!   `|value - median| * 255 > threshold`, i.e. the difference exceeds
//!   `threshold` 8-bit levels; a threshold of 0 replaces every channel
//!   that differs at all. Each channel decides alone.
//! * Alpha is never changed. A pixel with no alpha, or a non-finite
//!   sample, is left exactly as stored. Replaced values are
//!   re-premultiplied by the pixel's own alpha.
//!
//! The median reads the window as it was before the pass (the output is
//! written to the window afterwards, never fed back), every comparison is
//! on `f32` values and the sort is total, so the result at a pixel depends
//! only on the window content within `radius` and is bit-identical
//! however the window is cut: [`aurora_core::FilterKind::reach`] is the
//! radius. The outer ring of the window, `radius` wide, sees a truncated
//! disc and is not exact; callers crop it away.

use aurora_core::MAX_DUST_RADIUS;

const CHANNELS: usize = 4;

/// Runs Dust & Scratches over a `side` x `side` window of premultiplied
/// RGBA `f32` in place. `radius` is clamped to `1..=MAX_DUST_RADIUS`
/// (`0` changes nothing). A window whose length is not
/// `side * side * 4` changes nothing.
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::many_single_char_names
)]
pub fn dust_scratches_window(window: &mut [f32], side: usize, radius: u32, threshold: u8) {
    let Some(len) = side.checked_mul(side).and_then(|n| n.checked_mul(CHANNELS)) else {
        return;
    };
    if len != window.len() || radius == 0 || side == 0 {
        return;
    }
    let radius = i64::from(radius.min(MAX_DUST_RADIUS));
    let limit = f32::from(threshold);
    // Straight colour of every pixel that can take part, as three planes
    // plus a running count of valid pixels per row (so a fully valid run
    // is copied as a slice).
    let n = side * side;
    let mut planes: [Vec<f32>; 3] = [vec![0.0; n], vec![0.0; n], vec![0.0; n]];
    let mut valid = vec![false; n];
    for (i, texel) in window.chunks_exact(CHANNELS).enumerate() {
        let (Some(&a), Some(&r), Some(&g), Some(&b)) =
            (texel.get(3), texel.first(), texel.get(1), texel.get(2))
        else {
            continue;
        };
        if !(a.is_finite() && a > 0.0 && r.is_finite() && g.is_finite() && b.is_finite()) {
            continue;
        }
        let c = [r / a, g / a, b / a];
        if !c.iter().all(|v| v.is_finite()) {
            continue;
        }
        if let Some(slot) = valid.get_mut(i) {
            *slot = true;
        }
        for (plane, v) in planes.iter_mut().zip(c) {
            if let Some(slot) = plane.get_mut(i) {
                *slot = v;
            }
        }
    }
    let mut run_count = vec![0_u32; n + side];
    for y in 0..side {
        let mut acc = 0_u32;
        for x in 0..side {
            acc += u32::from(valid.get(y * side + x).copied().unwrap_or(false));
            if let Some(slot) = run_count.get_mut(y * (side + 1) + x + 1) {
                *slot = acc;
            }
        }
    }
    // Half-width of the disc on each row offset.
    let half: Vec<i64> = (-radius..=radius)
        .map(|dy| {
            let mut w = 0_i64;
            while (w + 1) * (w + 1) + dy * dy <= radius * radius {
                w += 1;
            }
            w
        })
        .collect();
    let side_i = side as i64;
    let mut buf: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for y in 0..side_i {
        for x in 0..side_i {
            let index = (y * side_i + x) as usize;
            if !valid.get(index).copied().unwrap_or(false) {
                continue;
            }
            let own = [0, 1, 2].map(|c| planes.get(c).and_then(|p| p.get(index)).copied());
            for b in &mut buf {
                b.clear();
            }
            for (k, &hw) in half.iter().enumerate() {
                let ny = y + k as i64 - radius;
                if !(0..side_i).contains(&ny) {
                    continue;
                }
                let x_lo = (x - hw).max(0) as usize;
                let x_hi = (x + hw).min(side_i - 1) as usize;
                let row = ny as usize * side;
                let counts = ny as usize * (side + 1);
                let found = match (
                    run_count.get(counts + x_hi + 1),
                    run_count.get(counts + x_lo),
                ) {
                    (Some(hi), Some(lo)) => (hi - lo) as usize,
                    _ => continue,
                };
                if found == 0 {
                    continue;
                }
                let whole = found == x_hi - x_lo + 1;
                for (b, plane) in buf.iter_mut().zip(&planes) {
                    let Some(cells) = plane.get(row + x_lo..=row + x_hi) else {
                        continue;
                    };
                    if whole {
                        b.extend_from_slice(cells);
                    } else {
                        for (offset, &v) in cells.iter().enumerate() {
                            if valid.get(row + x_lo + offset).copied().unwrap_or(false) {
                                b.push(v);
                            }
                        }
                    }
                }
            }
            let Some(texel) = window.get_mut(index * CHANNELS..index * CHANNELS + CHANNELS) else {
                continue;
            };
            let a = texel.get(3).copied().unwrap_or(0.0);
            for ((slot, b), value) in texel.iter_mut().zip(buf.iter_mut()).zip(own) {
                let Some(value) = value else { continue };
                if b.is_empty() {
                    continue;
                }
                let mid = (b.len() - 1) / 2;
                let (_, median, _) = b.select_nth_unstable_by(mid, f32::total_cmp);
                let median = *median;
                if (value - median).abs() * 255.0 > limit {
                    *slot = median * a;
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::many_single_char_names,
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
mod tests {
    use super::*;

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

    fn grey(v: f32) -> [f32; 4] {
        [v, v, v, 1.0]
    }

    #[test]
    fn a_speck_on_a_flat_field_is_removed() {
        let side = 21;
        let mut w = window(side, |x, y| {
            if (x, y) == (10, 10) {
                grey(1.0)
            } else {
                grey(0.3)
            }
        });
        dust_scratches_window(&mut w, side, 1, 0);
        for c in 0..3 {
            assert_eq!(at(&w, side, 10, 10, c), 0.3);
        }
        assert_eq!(at(&w, side, 10, 10, 3), 1.0);
        // A larger radius removes a 2 x 2 blob that radius 1 cannot.
        let blob = |x: usize, y: usize| {
            if (9..11).contains(&x) && (9..11).contains(&y) {
                grey(1.0)
            } else {
                grey(0.3)
            }
        };
        let mut r1 = window(side, blob);
        dust_scratches_window(&mut r1, side, 1, 0);
        assert_eq!(
            at(&r1, side, 9, 9, 0),
            1.0,
            "radius 1 keeps a 2x2 blob corner"
        );
        let mut r2 = window(side, blob);
        dust_scratches_window(&mut r2, side, 2, 0);
        for (x, y) in [(9, 9), (10, 9), (9, 10), (10, 10)] {
            assert_eq!(at(&r2, side, x, y, 0), 0.3);
        }
    }

    #[test]
    fn a_speck_within_the_threshold_is_kept_and_one_beyond_it_goes() {
        let side = 15;
        let make = |level: f32| {
            window(side, |x, y| {
                if (x, y) == (7, 7) {
                    grey(0.3 + level / 255.0)
                } else {
                    grey(0.3)
                }
            })
        };
        // 10 levels with threshold 10 is not "more than": kept (the
        // difference is not exactly 10 in f32, so use a clear margin).
        let mut kept = make(9.5);
        dust_scratches_window(&mut kept, side, 1, 10);
        assert!(at(&kept, side, 7, 7, 0) > 0.3 + 9.0 / 255.0);
        let mut gone = make(11.0);
        dust_scratches_window(&mut gone, side, 1, 10);
        assert_eq!(at(&gone, side, 7, 7, 0), 0.3);
        // Threshold 0 replaces even a one-level difference.
        let mut tiny = make(1.0);
        dust_scratches_window(&mut tiny, side, 1, 0);
        assert_eq!(at(&tiny, side, 7, 7, 0), 0.3);
    }

    #[test]
    fn each_channel_decides_alone() {
        let side = 15;
        let mut w = window(side, |x, y| {
            if (x, y) == (7, 7) {
                [1.0, 0.3 + 2.0 / 255.0, 0.3, 1.0]
            } else {
                [0.3, 0.3, 0.3, 1.0]
            }
        });
        dust_scratches_window(&mut w, side, 1, 5);
        assert_eq!(at(&w, side, 7, 7, 0), 0.3, "red beyond threshold goes");
        assert!(at(&w, side, 7, 7, 1) > 0.3, "green within it stays");
    }

    #[test]
    fn a_large_step_edge_survives() {
        let side = 41;
        let src = window(side, |x, _| if x < 20 { grey(0.2) } else { grey(0.8) });
        let mut w = src.clone();
        dust_scratches_window(&mut w, side, 3, 0);
        // Away from the window's truncated ring the edge is untouched.
        for y in 4..side - 4 {
            for x in 4..side - 4 {
                assert_eq!(at(&w, side, x, y, 0), at(&src, side, x, y, 0), "{x},{y}");
            }
        }
    }

    #[test]
    fn transparent_pixels_stay_and_do_not_bleed() {
        let side = 21;
        // Left half transparent with garbage colour stored premultiplied
        // as zero; right half opaque green with one red speck.
        let mut w = window(side, |x, y| {
            if x < 10 {
                [0.0, 0.0, 0.0, 0.0]
            } else if (x, y) == (15, 10) {
                [1.0, 0.0, 0.0, 1.0]
            } else {
                [0.0, 0.8, 0.0, 1.0]
            }
        });
        let before = w.clone();
        dust_scratches_window(&mut w, side, 2, 0);
        for y in 0..side {
            for x in 0..10 {
                for c in 0..4 {
                    assert_eq!(
                        at(&w, side, x, y, c).to_bits(),
                        at(&before, side, x, y, c).to_bits()
                    );
                }
            }
        }
        // The column next to the transparent edge keeps its green: the
        // transparent neighbours did not drag it towards black.
        assert_eq!(at(&w, side, 10, 10, 1), 0.8);
        assert_eq!(at(&w, side, 15, 10, 0), 0.0, "speck removed");
        assert_eq!(at(&w, side, 15, 10, 1), 0.8);
    }

    #[test]
    fn nan_is_never_chosen_and_non_finite_pixels_stay() {
        let side = 11;
        let mut w = window(side, |x, y| {
            if (x, y) == (5, 5) {
                grey(1.0)
            } else {
                grey(0.4)
            }
        });
        let nan_at = (5 * side + 6) * 4;
        if let Some(s) = w.get_mut(nan_at) {
            *s = f32::NAN;
        }
        dust_scratches_window(&mut w, side, 2, 0);
        assert_eq!(at(&w, side, 5, 5, 0), 0.4);
        assert!(
            at(&w, side, 6, 5, 0).is_nan(),
            "the NaN pixel is left as stored"
        );
        assert!(
            w.iter()
                .enumerate()
                .all(|(i, v)| v.is_finite() || i == nan_at)
        );
    }

    #[test]
    fn the_median_is_a_sample_value_the_lower_one_for_an_even_count() {
        let side = 9;
        // Row of 4 distinct values and a transparent hole making n even:
        // use radius 1 (5 pixels) with the left neighbour transparent.
        let mut w = window(side, |x, y| match (x, y) {
            (3, 4) => [0.0; 4],
            (4, 4) => grey(0.9),
            (5, 4) => grey(0.1),
            (4, 3) => grey(0.5),
            (4, 5) => grey(0.7),
            _ => grey(0.0),
        });
        dust_scratches_window(&mut w, side, 1, 0);
        // Values 0.9 (self), 0.1, 0.5, 0.7: sorted 0.1 0.5 0.7 0.9, index 1.
        assert_eq!(at(&w, side, 4, 4, 0), 0.5);
    }

    #[test]
    fn reach_is_the_radius_and_windows_agree_bit_for_bit() {
        let big = 80_usize;
        let source = window(big, |x, y| {
            let a = if (x / 7 + y / 5) % 6 == 0 { 0.5 } else { 1.0 };
            let v = 0.5 + 0.4 * noise((y * big + x) as u64);
            [
                v * a,
                (0.4 + 0.3 * noise((x * big + y + 1) as u64)) * a,
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
        for (radius, threshold) in [(1_u32, 0_u8), (4, 6), (9, 0)] {
            let reach = radius as usize;
            let run = |ox, oy, side| {
                let mut w = cut(ox, oy, side);
                dust_scratches_window(&mut w, side, radius, threshold);
                w
            };
            let (a, b) = (run(5, 5, 50), run(12, 9, 46));
            let mut compared = 0;
            for gy in 5..55_usize {
                for gx in 5..55_usize {
                    let ok = |g: usize, o: usize, s: usize| g >= o + reach && g < o + s - reach;
                    if !(ok(gx, 5, 50) && ok(gy, 5, 50) && ok(gx, 12, 46) && ok(gy, 9, 46)) {
                        continue;
                    }
                    for c in 0..4 {
                        assert_eq!(
                            at(&a, 50, gx - 5, gy - 5, c).to_bits(),
                            at(&b, 46, gx - 12, gy - 9, c).to_bits()
                        );
                    }
                    compared += 1;
                }
            }
            assert!(compared > 100, "{compared}");
            // Poke a neighbour just outside the disc (distance radius+1)
            // and just inside (distance radius) of the centre pixel.
            let (px, py, n) = (30_usize, 30_usize, 61_usize);
            let base = {
                let mut w = cut(0, 0, n);
                dust_scratches_window(&mut w, n, radius, threshold);
                w
            };
            let poke = |dx: usize| {
                let mut w = cut(0, 0, n);
                let i = (py * n + px + dx) * 4;
                for k in 0..3 {
                    if let Some(s) = w.get_mut(i + k) {
                        *s = 5.0;
                    }
                }
                dust_scratches_window(&mut w, n, radius, threshold);
                w
            };
            let outside = poke(reach + 1);
            for c in 0..4 {
                assert_eq!(
                    at(&outside, n, px, py, c).to_bits(),
                    at(&base, n, px, py, c).to_bits()
                );
            }
        }
    }

    /// Cost of one 256 x 256 tile at several radii, printed (run with
    /// `--ignored --nocapture`).
    #[test]
    #[ignore = "timing"]
    fn timing_of_one_tile() {
        for radius in [1_u32, 5, 8, 10, 12, 15, 20] {
            let side = 256 + 2 * radius as usize;
            let base = window(side, |x, y| grey(0.5 + 0.3 * noise((y * side + x) as u64)));
            let mut times = Vec::new();
            for _ in 0..3 {
                let mut w = base.clone();
                let start = std::time::Instant::now();
                dust_scratches_window(&mut w, side, radius, 0);
                times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            println!("radius {radius}: ms per tile {times:?}");
        }
    }
}
