//! The Gradient Map adjustment's pixel math (0.248.0).
//!
//! [`GradientMapOp`] applies an [`aurora_core::GradientMapParams`] to
//! **straight (un-premultiplied) `f32` RGB**, encoded sRGB in and out, alpha
//! never an input or output.
//!
//! # The formula
//!
//! 1. `grey = 0.30 R + 0.59 G + 0.11 B` ([`crate::threshold::LUMA`], the
//!    weights measured against Photoshop for Threshold; **not measured for
//!    Gradient Map**, see below) on the input clamped to `[0, 1]` (`NaN`
//!    reads as `0`);
//! 2. with Reverse, `t = 1 - grey`, else `t = grey`;
//! 3. the colour of the gradient at `t`: the first stop's colour at or
//!    before the first stop, the last stop's at or after the last, and in
//!    between the linear interpolation, **in encoded sRGB**, of the two
//!    stops around `t` (Photoshop's Classic interpolation). At a position
//!    shared by two stops the later stop wins.
//!
//! The output is a convex combination of stop colours, so it is always in
//! `[0, 1]` and no clamp is needed.
//!
//! # What is and is not validated
//!
//! **Nothing here is compared with Photoshop's pixels.** The only Gradient
//! Map layers in the corpus (`layers/gradient-map*.psd`) sit over no pixels
//! (their merged image is empty) and the one in `fill_adjustments.psd` is
//! hidden, so no fixture holds a gradient-mapped picture. The grey weights
//! are the Threshold measurement carried over by assumption; the Classic
//! interpolation in encoded sRGB is Photoshop's documented default for the
//! method byte `Gcls`. Dither is accepted and ignored (there is no 8-bit
//! quantisation here to dither).
//!
//! # Cost
//!
//! A binary search over at most 64 stops per pixel; nothing is tabulated, so
//! a hard step is exact.

use crate::threshold::LUMA;
use aurora_core::{GRADIENT_MAP_MAX_STOPS, GradientMapParams};

/// How many `f32`s [`GradientMapOp::packed_for_gpu`] returns (0.250.0): a
/// header `vec4` and [`aurora_core::GRADIENT_MAP_MAX_STOPS`] stops of four.
pub const GRADIENT_MAP_PACKED_LEN: usize = 4 + 4 * GRADIENT_MAP_MAX_STOPS;

/// A Gradient Map adjustment ready to apply to pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct GradientMapOp {
    /// `[position, red, green, blue]`, in order of position.
    stops: Vec<[f32; 4]>,
    reverse: bool,
}

impl GradientMapOp {
    /// Prepares `params` for application.
    #[must_use]
    pub fn new(params: &GradientMapParams) -> Self {
        Self {
            stops: params
                .stops()
                .iter()
                .map(|s| [s.position, s.colour[0], s.colour[1], s.colour[2]])
                .collect(),
            reverse: params.reverse(),
        }
    }

    /// Whether the map changes nothing: never. Even black-to-white turns a
    /// coloured pixel grey.
    #[must_use]
    pub const fn is_identity(&self) -> bool {
        false
    }

    /// The settings flattened for the GPU Gradient Map pass (0.250.0,
    /// `aurora_render::TileCompositor::composite_gradient_map_with_opacity`,
    /// `shaders/gradient_map.wgsl`): `[reverse, stop count, 0, 0]` then
    /// [`GRADIENT_MAP_MAX_STOPS`] stops of `[position, red, green, blue]`,
    /// in order of position (the very `f32`s [`Self::colour_at`] reads).
    /// Slots past the count repeat the last stop so no slot is ever
    /// uninitialised; the shader never reads them.
    #[must_use]
    pub fn packed_for_gpu(&self) -> [f32; GRADIENT_MAP_PACKED_LEN] {
        let mut out = [0.0_f32; GRADIENT_MAP_PACKED_LEN];
        let count = self.stops.len().min(GRADIENT_MAP_MAX_STOPS);
        out[0] = f32::from(u8::from(self.reverse));
        out[1] = u8::try_from(count).map_or(0.0, f32::from);
        let last = self.stops.last().copied().unwrap_or([0.0; 4]);
        for slot in 0..GRADIENT_MAP_MAX_STOPS {
            let stop = if slot < count {
                self.stops.get(slot).copied().unwrap_or(last)
            } else {
                last
            };
            for (j, v) in stop.into_iter().enumerate() {
                if let Some(cell) = out.get_mut(4 + slot * 4 + j) {
                    *cell = v;
                }
            }
        }
        out
    }

    /// The gradient's colour at `t` (`0..=1`; a value outside is clamped).
    #[must_use]
    pub fn colour_at(&self, t: f32) -> [f32; 3] {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        // The number of stops at or before `t`.
        let after = self.stops.partition_point(|s| s[0] <= t);
        let rgb = |s: &[f32; 4]| [s[1], s[2], s[3]];
        if after == 0 {
            return self.stops.first().map_or([0.0; 3], rgb);
        }
        match (self.stops.get(after - 1), self.stops.get(after)) {
            (Some(a), Some(b)) => {
                // `a.position <= t < b.position`, so the span is positive.
                let u = (t - a[0]) / (b[0] - a[0]);
                [
                    a[1] + (b[1] - a[1]) * u,
                    a[2] + (b[2] - a[2]) * u,
                    a[3] + (b[3] - a[3]) * u,
                ]
            }
            // `t` is at or past the last stop.
            (Some(a), None) => rgb(a),
            _ => [0.0; 3],
        }
    }

    /// The mapped colour of one straight, encoded RGB pixel.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let [r, g, b] = rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        let grey = (LUMA[0] * r + LUMA[1] * g + LUMA[2] * b).clamp(0.0, 1.0);
        self.colour_at(if self.reverse { 1.0 - grey } else { grey })
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::indexing_slicing)]
mod tests {
    use super::GradientMapOp;
    use aurora_core::{GradientMapParams, GradientStop};

    fn params(stops: &[(f32, [f32; 3])], reverse: bool) -> GradientMapParams {
        let stops = stops
            .iter()
            .map(|&(p, c)| GradientStop::new(p, c))
            .collect();
        match GradientMapParams::new(stops, reverse, false) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// An independent model: scan the segments in order.
    fn model(stops: &[(f32, [f32; 3])], reverse: bool, rgb: [f32; 3]) -> [f32; 3] {
        let c = |v: f32| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
        let grey = (0.30 * c(rgb[0]) + 0.59 * c(rgb[1]) + 0.11 * c(rgb[2])).clamp(0.0, 1.0);
        let t = if reverse { 1.0 - grey } else { grey };
        if t < stops[0].0 {
            return stops[0].1;
        }
        for pair in stops.windows(2) {
            let ((p0, c0), (p1, c1)) = (pair[0], pair[1]);
            if t >= p0 && t < p1 {
                let u = (t - p0) / (p1 - p0);
                return [0, 1, 2].map(|i| c0[i] + (c1[i] - c0[i]) * u);
            }
        }
        stops[stops.len() - 1].1
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-6)
    }

    const FIVE: [(f32, [f32; 3]); 5] = [
        (0.1, [0.2, 0.0, 0.4]),
        (0.3, [0.9, 0.1, 0.1]),
        (0.5, [0.5, 0.5, 0.0]),
        (0.5, [0.0, 0.8, 0.8]),
        (0.9, [1.0, 1.0, 0.9]),
    ];

    #[test]
    fn black_to_white_maps_grey_to_itself_and_colour_to_its_luma() {
        let op = GradientMapOp::new(&GradientMapParams::black_to_white());
        assert!(!op.is_identity());
        for v in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            assert!(close(op.apply([v; 3]), [v; 3]), "{v}");
        }
        // Pure red has grey 0.30.
        assert!(close(op.apply([1.0, 0.0, 0.0]), [0.30; 3]));
        assert!(close(op.apply([0.0, 1.0, 0.0]), [0.59; 3]));
        assert!(close(op.apply([0.0, 0.0, 1.0]), [0.11; 3]));
    }

    #[test]
    fn matches_an_independent_model_over_a_grid_for_both_directions() {
        for reverse in [false, true] {
            let op = GradientMapOp::new(&params(&FIVE, reverse));
            let steps = [0.0_f32, 0.07, 0.19, 0.33, 0.5, 0.51, 0.66, 0.8, 0.93, 1.0];
            for &r in &steps {
                for &g in &steps {
                    for &b in &steps {
                        let got = op.apply([r, g, b]);
                        let want = model(&FIVE, reverse, [r, g, b]);
                        assert!(
                            close(got, want),
                            "{r} {g} {b} rev {reverse}: {got:?} {want:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn reverse_mirrors_the_grey_axis() {
        let fwd = GradientMapOp::new(&params(&FIVE, false));
        let rev = GradientMapOp::new(&params(&FIVE, true));
        for v in [0.0_f32, 0.2, 0.45, 0.8, 1.0] {
            assert!(close(rev.apply([v; 3]), fwd.apply([1.0 - v; 3])), "{v}");
        }
    }

    #[test]
    fn colours_before_the_first_and_after_the_last_stop_hold_the_end_colours() {
        let op = GradientMapOp::new(&params(&FIVE, false));
        assert_eq!(op.colour_at(0.0), [0.2, 0.0, 0.4]);
        assert_eq!(op.colour_at(0.1), [0.2, 0.0, 0.4]);
        assert_eq!(op.colour_at(0.9), [1.0, 1.0, 0.9]);
        assert_eq!(op.colour_at(1.0), [1.0, 1.0, 0.9]);
    }

    #[test]
    fn two_stops_at_one_position_make_a_hard_step_and_the_later_wins() {
        let op = GradientMapOp::new(&params(&FIVE, false));
        // Just below 0.5 the 0.3..0.5 segment is nearly its end colour; at
        // 0.5 and after, the second 0.5 stop's colour begins.
        assert!(
            close(op.colour_at(0.499_999), [0.5, 0.5, 0.0]) || {
                let c = op.colour_at(0.499_999);
                (c[0] - 0.5).abs() < 1e-3 && (c[1] - 0.5).abs() < 1e-3
            }
        );
        assert_eq!(op.colour_at(0.5), [0.0, 0.8, 0.8]);
        let after = op.colour_at(0.500_001);
        assert!((after[1] - 0.8).abs() < 1e-3 && after[0] < 1e-3);
    }

    #[test]
    fn interpolation_is_linear_in_encoded_srgb_not_in_light() {
        let op = GradientMapOp::new(&params(&[(0.0, [0.0; 3]), (1.0, [1.0; 3])], false));
        // Halfway between black and white is encoded 0.5, not the 0.735 a
        // linear-light blend would give.
        assert!(close(op.colour_at(0.5), [0.5; 3]));
        let op = GradientMapOp::new(&params(
            &[(0.2, [1.0, 0.0, 0.0]), (0.6, [0.0, 0.0, 1.0])],
            false,
        ));
        assert!(close(op.colour_at(0.4), [0.5, 0.0, 0.5]));
    }

    #[test]
    fn out_of_range_and_nan_input_cannot_leave_the_gradient() {
        let op = GradientMapOp::new(&params(&FIVE, false));
        let lo = op.apply([-5.0; 3]);
        let hi = op.apply([5.0; 3]);
        assert_eq!(lo, [0.2, 0.0, 0.4]);
        assert_eq!(hi, [1.0, 1.0, 0.9]);
        assert_eq!(op.apply([f32::NAN; 3]), lo, "NaN reads as black");
        assert_eq!(op.colour_at(f32::NAN), lo);
        assert_eq!(op.colour_at(7.0), hi);
        assert_eq!(op.colour_at(-7.0), lo);
        for v in [-3.0_f32, 0.3, 9.0] {
            for c in op.apply([v, 0.5 * v, 0.1]) {
                assert!((0.0..=1.0).contains(&c));
            }
        }
    }

    #[test]
    fn dither_changes_no_pixel() {
        let stops: Vec<GradientStop> = FIVE.iter().map(|&(p, c)| GradientStop::new(p, c)).collect();
        let (Ok(a), Ok(b)) = (
            GradientMapParams::new(stops.clone(), false, false),
            GradientMapParams::new(stops, false, true),
        ) else {
            unreachable!("valid");
        };
        let (a, b) = (GradientMapOp::new(&a), GradientMapOp::new(&b));
        for v in [0.0_f32, 0.3, 0.6, 1.0] {
            assert_eq!(a.apply([v, 0.4, 0.9]), b.apply([v, 0.4, 0.9]));
        }
    }

    #[test]
    #[ignore = "timing, run with --release -- --ignored --nocapture"]
    fn cost_per_pixel_on_a_4096_square() {
        let stops: Vec<(f32, [f32; 3])> = (0..8)
            .map(|i| (i as f32 / 7.0, [i as f32 / 7.0, 0.5, 1.0 - i as f32 / 7.0]))
            .collect();
        let op = GradientMapOp::new(&params(&stops, false));
        let n = 4096_usize * 4096;
        let start = std::time::Instant::now();
        let mut acc = 0.0_f32;
        for i in 0..n {
            let v = (i % 251) as f32 / 250.0;
            let c = op.apply([v, 1.0 - v, (v * 3.0).fract()]);
            acc += c[0] + c[1] + c[2];
        }
        let ns = start.elapsed().as_nanos() as f64 / n as f64;
        println!("GRADIENT-MAP 8 stops: {ns:.1} ns/px (checksum {acc})");
    }
}
