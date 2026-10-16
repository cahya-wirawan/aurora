//! The Selective Color adjustment's pixel math (0.245.0).
//!
//! [`SelectiveColorOp`] applies an [`aurora_core::SelectiveColorParams`] to
//! **straight (un-premultiplied) `f32` RGB**, encoded sRGB in and out,
//! alpha never an input or output.
//!
//! # UNVALIDATED approximation
//!
//! **Nothing here was compared with Photoshop.** The corpus holds only
//! neutral Selective Color layers (every slider zero), whose merged images
//! the identity reproduces exactly; no fixture constrains a single weight
//! or ink formula below. Photoshop's own algorithm is unpublished (it works
//! in the CMYK of the document's profile). This is the textbook model,
//! chosen for being explicit and testable, and the open report says so.
//!
//! # The model
//!
//! 1. **Range weights** ([`range_weights`]), nine numbers summing to 1.
//!    The pixel is `max` (the largest channel), `mid`, `min`. Its
//!    chromatic content `chroma = max - min` splits between the primary
//!    nearest the largest channel and the secondary between it and the mid
//!    channel: `max - mid` goes to Reds (R largest), Greens or Blues, and
//!    `mid - min` to the secondary hue the mid channel names (Yellows,
//!    Magentas or Cyans); the six hue weights sum to `chroma`. The
//!    remaining `1 - chroma` is achromatic and splits by the HSL lightness
//!    `L = (max + min) / 2` into Whites `max(2L - 1, 0)`, Blacks
//!    `max(1 - 2L, 0)` and Neutrals `1 - |2L - 1|`. A pure primary or
//!    secondary is only its own hue; white only Whites; black only Blacks;
//!    mid grey only Neutrals.
//! 2. **Ink amounts** are the plain CMY complements `c = 1 - r`, `m = 1 -
//!    g`, `y = 1 - b` (no gamut or profile conversion) and the black
//!    amount `k = min(c, m, y)`.
//! 3. **The adjustment.** With `s_j = sum over ranges of weight * slider_j
//!    / 100` for each of cyan, magenta, yellow and black, **Relative**
//!    changes an ink by a fraction of itself, `dc = c * s_c` (so it cannot
//!    add ink where there is none: pure red has no cyan), `dk = k * s_k`;
//!    **Absolute** by a fixed amount, `dc = s_c`, `dk = s_k`. The black
//!    change applies to all three channels (`k` is the grey component they
//!    share). The new red is `1 - clamp(c + dc + dk, 0, 1)`, likewise green
//!    with `m + dm + dk` and blue with `y + dy + dk`.
//! 4. A channel whose own ink change and black change are both exactly zero
//!    is returned as the clamped input, so an all-zero adjustment is the
//!    identity bit for bit.
//!
//! Input is clamped to `[0, 1]` (`NaN` reads as `0`).

// Colour channels and inks are named `r g b` and `c m y k`, as in the docs.
#![allow(clippy::many_single_char_names)]

use aurora_core::SelectiveColorParams;

/// The nine range weights of one straight, encoded pixel, in the
/// parameters' order (Reds, Yellows, Greens, Cyans, Blues, Magentas,
/// Whites, Neutrals, Blacks). Non-negative, summing to 1 (within `f32`
/// rounding). See the module docs.
#[must_use]
pub fn range_weights(rgb: [f32; 3]) -> [f32; 9] {
    let v = rgb.map(|c| if c.is_nan() { 0.0 } else { c.clamp(0.0, 1.0) });
    let [r, g, b] = v;
    // The index of the largest channel and of the middle one.
    let (hi, mid) = if r >= g {
        if g >= b {
            (0, 1)
        } else if r >= b {
            (0, 2)
        } else {
            (2, 0)
        }
    } else if r >= b {
        (1, 0)
    } else if g >= b {
        (1, 2)
    } else {
        (2, 1)
    };
    let at = |i: usize| v.get(i).copied().unwrap_or(0.0);
    let (max, mid_v) = (at(hi), at(mid));
    let min = r.min(g).min(b);
    let chroma = max - min;
    // The primary the largest channel names, and the secondary between it
    // and the middle channel.
    let primary = match hi {
        0 => 0,
        1 => 2,
        _ => 4,
    };
    let secondary = match (hi, mid) {
        (0, 1) | (1, 0) => 1,
        (1, 2) | (2, 1) => 3,
        _ => 5,
    };
    let mut w = [0.0_f32; 9];
    if let Some(slot) = w.get_mut(primary) {
        *slot += max - mid_v;
    }
    if let Some(slot) = w.get_mut(secondary) {
        *slot += mid_v - min;
    }
    let achromatic = 1.0 - chroma;
    let tone = max + min - 1.0;
    if let Some(slot) = w.get_mut(6) {
        *slot = achromatic * tone.max(0.0);
    }
    if let Some(slot) = w.get_mut(7) {
        *slot = achromatic * (1.0 - tone.abs());
    }
    if let Some(slot) = w.get_mut(8) {
        *slot = achromatic * (-tone).max(0.0);
    }
    w
}

/// How many `f32`s [`SelectiveColorOp::packed_for_gpu`] returns (0.250.0).
pub const SELECTIVE_COLOR_PACKED_LEN: usize = 40;

/// A Selective Color adjustment ready to apply to pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectiveColorOp {
    /// The sliders as fractions (`percent / 100`), `[cyan, magenta, yellow,
    /// black]` per range.
    sliders: [[f32; 4]; 9],
    absolute: bool,
    identity: bool,
}

impl SelectiveColorOp {
    /// Prepares `params` for application.
    #[must_use]
    pub fn new(params: &SelectiveColorParams) -> Self {
        Self {
            sliders: params.ranges().map(|row| row.map(|v| v / 100.0)),
            absolute: params.absolute(),
            identity: params.is_identity(),
        }
    }

    /// Whether every slider is zero.
    #[must_use]
    pub const fn is_identity(&self) -> bool {
        self.identity
    }

    /// The settings flattened for the GPU Selective Color pass (0.250.0,
    /// `aurora_render::TileCompositor::composite_selective_color_with_opacity`,
    /// `shaders/selective_color.wgsl`): `[absolute, 0, 0, 0]` then the nine
    /// ranges (Reds, Yellows, Greens, Cyans, Blues, Magentas, Whites,
    /// Neutrals, Blacks) of `[cyan, magenta, yellow, black]` as fractions,
    /// the very `f32`s [`Self::apply`] weighs. No identity flag is sent: an
    /// all-zero adjustment passes every channel through on the general path
    /// (the `delta == 0 && d_k == 0` rule), exactly as the CPU's early
    /// return does.
    #[must_use]
    pub fn packed_for_gpu(&self) -> [f32; SELECTIVE_COLOR_PACKED_LEN] {
        let mut out = [0.0_f32; SELECTIVE_COLOR_PACKED_LEN];
        out[0] = f32::from(u8::from(self.absolute));
        for (i, v) in self.sliders.iter().flatten().enumerate() {
            if let Some(slot) = out.get_mut(4 + i) {
                *slot = *v;
            }
        }
        out
    }

    /// The adjusted colour of one straight, encoded RGB pixel.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let [r, g, b] = rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        if self.identity {
            return [r, g, b];
        }
        let weights = range_weights([r, g, b]);
        let mut s = [0.0_f32; 4];
        for (w, row) in weights.iter().zip(&self.sliders) {
            // At most three ranges weigh anything: skip the rest.
            if *w == 0.0 {
                continue;
            }
            for (sum, slider) in s.iter_mut().zip(row) {
                *sum = w.mul_add(*slider, *sum);
            }
        }
        let [s_c, s_m, s_y, s_k] = s;
        let (c, m, y) = (1.0 - r, 1.0 - g, 1.0 - b);
        let k = c.min(m).min(y);
        let (d_c, d_m, d_y, d_k) = if self.absolute {
            (s_c, s_m, s_y, s_k)
        } else {
            (c * s_c, m * s_m, y * s_y, k * s_k)
        };
        let channel = |value: f32, ink: f32, delta: f32| {
            if delta == 0.0 && d_k == 0.0 {
                value
            } else {
                1.0 - (ink + delta + d_k).clamp(0.0, 1.0)
            }
        };
        [channel(r, c, d_c), channel(g, m, d_m), channel(b, y, d_y)]
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::indexing_slicing)]
mod tests {
    use super::{SelectiveColorOp, range_weights};
    use aurora_core::SelectiveColorParams;

    const REDS: usize = 0;
    const YELLOWS: usize = 1;
    const GREENS: usize = 2;
    const CYANS: usize = 3;
    const BLUES: usize = 4;
    const MAGENTAS: usize = 5;
    const WHITES: usize = 6;
    const NEUTRALS: usize = 7;
    const BLACKS: usize = 8;

    /// One range with the given sliders, Relative or Absolute.
    fn op(range: usize, sliders: [f32; 4], absolute: bool) -> SelectiveColorOp {
        let mut ranges = [[0.0_f32; 4]; 9];
        ranges[range] = sliders;
        match SelectiveColorParams::new(ranges, absolute) {
            Ok(p) => SelectiveColorOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn grid() -> Vec<[f32; 3]> {
        let s: Vec<f32> = (0..=5_u8).map(|i| f32::from(i) / 5.0).collect();
        let mut out = Vec::new();
        for &r in &s {
            for &g in &s {
                for &b in &s {
                    out.push([r, g, b]);
                }
            }
        }
        out
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn the_nine_weights_are_non_negative_and_sum_to_one_everywhere() {
        for px in grid() {
            let w = range_weights(px);
            assert!(w.iter().all(|&x| x >= 0.0), "{px:?} {w:?}");
            assert!(near(w.iter().sum(), 1.0), "{px:?} {w:?}");
        }
    }

    #[test]
    fn the_pure_colours_black_white_and_grey_belong_to_exactly_one_range() {
        let cases: [([f32; 3], usize); 9] = [
            ([1.0, 0.0, 0.0], REDS),
            ([1.0, 1.0, 0.0], YELLOWS),
            ([0.0, 1.0, 0.0], GREENS),
            ([0.0, 1.0, 1.0], CYANS),
            ([0.0, 0.0, 1.0], BLUES),
            ([1.0, 0.0, 1.0], MAGENTAS),
            ([1.0, 1.0, 1.0], WHITES),
            ([0.5, 0.5, 0.5], NEUTRALS),
            ([0.0, 0.0, 0.0], BLACKS),
        ];
        for (px, want) in cases {
            let w = range_weights(px);
            for (i, x) in w.iter().enumerate() {
                let expected = if i == want { 1.0 } else { 0.0 };
                assert!(near(*x, expected), "{px:?} range {i}: {x}");
            }
        }
    }

    #[test]
    fn a_hue_between_two_names_splits_between_them_and_the_six_sectors_are_distinct() {
        // Orange is half red, half yellow; each sector's mid colour likewise.
        let sectors: [([f32; 3], usize, usize); 6] = [
            ([1.0, 0.5, 0.0], REDS, YELLOWS),
            ([0.5, 1.0, 0.0], GREENS, YELLOWS),
            ([0.0, 1.0, 0.5], GREENS, CYANS),
            ([0.0, 0.5, 1.0], BLUES, CYANS),
            ([0.5, 0.0, 1.0], BLUES, MAGENTAS),
            ([1.0, 0.0, 0.5], REDS, MAGENTAS),
        ];
        for (px, a, b) in sectors {
            let w = range_weights(px);
            assert!(near(w[a], 0.5) && near(w[b], 0.5), "{px:?} {w:?}");
        }
        // A quarter of the way: 3/4 and 1/4.
        let w = range_weights([1.0, 0.25, 0.0]);
        assert!(near(w[REDS], 0.75) && near(w[YELLOWS], 0.25), "{w:?}");
    }

    #[test]
    fn lightness_splits_the_achromatic_part_between_whites_neutrals_and_blacks() {
        let w = range_weights([0.75; 3]);
        assert!(near(w[WHITES], 0.5) && near(w[NEUTRALS], 0.5) && near(w[BLACKS], 0.0));
        let w = range_weights([0.25; 3]);
        assert!(near(w[BLACKS], 0.5) && near(w[NEUTRALS], 0.5) && near(w[WHITES], 0.0));
        // A pale pink: a fifth chromatic (red), four fifths achromatic.
        let w = range_weights([1.0, 0.8, 0.8]);
        assert!(near(w[REDS], 0.2), "{w:?}");
        assert!(near(w[WHITES] + w[NEUTRALS] + w[BLACKS], 0.8), "{w:?}");
        assert!(w[WHITES] > w[NEUTRALS] && w[BLACKS] == 0.0, "{w:?}");
    }

    #[test]
    fn all_zero_sliders_return_the_clamped_input_exactly() {
        for absolute in [false, true] {
            let ranges = [[0.0_f32; 4]; 9];
            let Ok(p) = SelectiveColorParams::new(ranges, absolute) else {
                unreachable!("valid");
            };
            let o = SelectiveColorOp::new(&p);
            assert!(o.is_identity());
            for px in grid() {
                assert_eq!(o.apply(px), px);
            }
            assert_eq!(o.apply([1.0 / 3.0, 0.1, 0.7]), [1.0 / 3.0, 0.1, 0.7]);
            assert_eq!(o.apply([9.0, -9.0, f32::NAN]), [1.0, 0.0, 0.0]);
        }
        assert!(!op(REDS, [0.0, 0.0, 0.0, 1.0], false).is_identity());
    }

    #[test]
    fn relative_scales_the_ink_a_pixel_has_and_cannot_add_ink_it_lacks() {
        // Blue (0,0,1) has cyan 1 and magenta 1, yellow 0.
        let cyan_down = op(BLUES, [-50.0, 0.0, 0.0, 0.0], false).apply([0.0, 0.0, 1.0]);
        assert!(near(cyan_down[0], 0.5) && cyan_down[1] == 0.0 && cyan_down[2] == 1.0);
        // Red has no cyan: a Relative cyan slider on Reds changes nothing.
        assert_eq!(
            op(REDS, [100.0, 0.0, 0.0, 0.0], false).apply([1.0, 0.0, 0.0]),
            [1.0, 0.0, 0.0]
        );
        // Half red (1, 0.5, 0.5): cyan 0, magenta 0.5. Magenta +50 % of 0.5
        // is +0.25 ink, so green falls from 0.5 to 0.25.
        let out = op(REDS, [0.0, 50.0, 0.0, 0.0], false).apply([1.0, 0.0, 0.0]);
        assert!(near(out[1], 0.0), "magenta is already 1 and clips: {out:?}");
        let pink = op(REDS, [0.0, 50.0, 0.0, 0.0], false).apply([1.0, 0.5, 0.5]);
        assert!(
            near(
                pink[1],
                0.5 - 0.5 * 0.5 * range_weights([1.0, 0.5, 0.5])[REDS]
            ),
            "{pink:?}"
        );
    }

    #[test]
    fn absolute_adds_a_fixed_amount_even_where_there_is_none() {
        // Pure red with absolute cyan +50 %: red loses half its red.
        let out = op(REDS, [50.0, 0.0, 0.0, 0.0], true).apply([1.0, 0.0, 0.0]);
        assert!(
            near(out[0], 0.5) && out[1] == 0.0 && out[2] == 0.0,
            "{out:?}"
        );
        // Negative absolute cyan on a cyan-free red clips at no ink.
        assert_eq!(
            op(REDS, [-50.0, 0.0, 0.0, 0.0], true).apply([1.0, 0.0, 0.0]),
            [1.0, 0.0, 0.0]
        );
        // The same slider in Relative does nothing there.
        assert_eq!(
            op(REDS, [50.0, 0.0, 0.0, 0.0], false).apply([1.0, 0.0, 0.0]),
            [1.0, 0.0, 0.0]
        );
    }

    #[test]
    fn the_black_slider_moves_all_three_channels_together() {
        // Mid grey is all black amount 0.5: Relative black +100 doubles it.
        let out = op(NEUTRALS, [0.0, 0.0, 0.0, 100.0], false).apply([0.5; 3]);
        assert!(
            near(out[0], 0.0) && out[0] == out[1] && out[1] == out[2],
            "{out:?}"
        );
        // Absolute black -20 % on mid grey lifts every channel by 0.2.
        let out = op(NEUTRALS, [0.0, 0.0, 0.0, -20.0], true).apply([0.5; 3]);
        assert!(
            near(out[0], 0.7) && near(out[1], 0.7) && near(out[2], 0.7),
            "{out:?}"
        );
        // Relative black on white (no black amount) changes nothing.
        assert_eq!(
            op(WHITES, [0.0, 0.0, 0.0, 100.0], false).apply([1.0; 3]),
            [1.0; 3]
        );
        // Absolute black on white darkens it.
        let out = op(WHITES, [0.0, 0.0, 0.0, 30.0], true).apply([1.0; 3]);
        assert!(near(out[0], 0.7), "{out:?}");
    }

    #[test]
    fn a_range_leaves_the_other_colours_alone_and_each_ink_moves_its_own_channel() {
        let o = op(BLUES, [-100.0, -100.0, 100.0, 100.0], true);
        for px in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0; 3],
            [0.5; 3],
        ] {
            let out = o.apply(px);
            for ch in 0..3 {
                assert!(near(out[ch], px[ch]), "{px:?} moved to {out:?}");
            }
        }
        // Each ink alone, Absolute, on pure blue's own range: cyan -> red up,
        // magenta -> green up, yellow -> blue down.
        let c = op(BLUES, [-40.0, 0.0, 0.0, 0.0], true).apply([0.0, 0.0, 1.0]);
        assert!(near(c[0], 0.4) && c[1] == 0.0 && c[2] == 1.0, "{c:?}");
        let m = op(BLUES, [0.0, -40.0, 0.0, 0.0], true).apply([0.0, 0.0, 1.0]);
        assert!(m[0] == 0.0 && near(m[1], 0.4) && m[2] == 1.0, "{m:?}");
        let y = op(BLUES, [0.0, 0.0, 40.0, 0.0], true).apply([0.0, 0.0, 1.0]);
        assert!(y[0] == 0.0 && y[1] == 0.0 && near(y[2], 0.6), "{y:?}");
    }

    #[test]
    fn it_matches_an_independent_model_over_a_colour_grid() {
        // A model written from the module docs with its own weights.
        fn model(sliders: [[f32; 4]; 9], absolute: bool, px: [f32; 3]) -> [f64; 3] {
            let v = px.map(|x| f64::from(x).clamp(0.0, 1.0));
            let (r, g, b) = (v[0], v[1], v[2]);
            let mx = r.max(g).max(b);
            let mn = r.min(g).min(b);
            let mut sorted = [r, g, b];
            sorted.sort_by(|a, b| b.total_cmp(a));
            let mid = sorted[1];
            let hi_i = if r >= g && r >= b {
                0
            } else if g >= b {
                1
            } else {
                2
            };
            let mut w = [0.0_f64; 9];
            // Largest channel by index, middle channel by value ordering.
            let secondary = {
                let rest: Vec<usize> = (0..3).filter(|&i| i != hi_i).collect();
                let mid_i = if v[rest[0]] >= v[rest[1]] {
                    rest[0]
                } else {
                    rest[1]
                };
                match (hi_i, mid_i) {
                    (0, 1) | (1, 0) => 1,
                    (1, 2) | (2, 1) => 3,
                    _ => 5,
                }
            };
            w[[0, 2, 4][hi_i]] += mx - mid;
            w[secondary] += mid - mn;
            let a = 1.0 - (mx - mn);
            let l2 = mx + mn - 1.0;
            w[6] = a * l2.max(0.0);
            w[7] = a * (1.0 - l2.abs());
            w[8] = a * (-l2).max(0.0);
            let mut s = [0.0_f64; 4];
            for i in 0..9 {
                for j in 0..4 {
                    s[j] += w[i] * f64::from(sliders[i][j]) / 100.0;
                }
            }
            let ink = [1.0 - r, 1.0 - g, 1.0 - b];
            let k = ink[0].min(ink[1]).min(ink[2]);
            let dk = if absolute { s[3] } else { k * s[3] };
            let mut out = [0.0; 3];
            for ch in 0..3 {
                let dc = if absolute { s[ch] } else { ink[ch] * s[ch] };
                out[ch] = if dc == 0.0 && dk == 0.0 {
                    v[ch]
                } else {
                    1.0 - (ink[ch] + dc + dk).clamp(0.0, 1.0)
                };
            }
            out
        }
        let mut sliders = [[0.0_f32; 4]; 9];
        for (i, row) in sliders.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (((i * 7 + j * 13) % 41) as f32 - 20.0) * 4.0;
            }
        }
        for absolute in [false, true] {
            let Ok(p) = SelectiveColorParams::new(sliders, absolute) else {
                unreachable!("valid");
            };
            let o = SelectiveColorOp::new(&p);
            for px in grid() {
                let got = o.apply(px);
                let want = model(sliders, absolute, px);
                for ch in 0..3 {
                    assert!(
                        (f64::from(got[ch]) - want[ch]).abs() < 1e-5,
                        "absolute {absolute} at {px:?} channel {ch}: {} vs {}",
                        got[ch],
                        want[ch]
                    );
                }
            }
        }
    }
}
