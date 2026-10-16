//! The Threshold adjustment's pixel math (0.244.0).
//!
//! [`ThresholdOp`] applies an [`aurora_core::ThresholdParams`] to
//! **straight (un-premultiplied) `f32` RGB**, encoded sRGB in and out,
//! alpha never an input or output. The formula is on the parameter type
//! (`aurora-core`'s `threshold` module): the grey `0.30 R + 0.59 G + 0.11 B`
//! is compared with the level; white at or above, black below.
//!
//! # Decisions, with reasons
//!
//! - **Photoshop's weights, not Rec.709** (this crate's other colour maths):
//!   fitted on the fixtures, 3 mismatching pixels of 40,000 against 553 for
//!   Rec.709 ([`LUMA`]).
//! - **The edge is `level - 0.5`** on the 0 to 255 scale ([`edge`]):
//!   Photoshop's 8-bit mode compares the *rounded* grey. A grey exactly on
//!   the edge is white; a pixel promoted from 8 bits can sit exactly there,
//!   where `f16` storage (not Photoshop's integer arithmetic) decides.
//! - **GPU (0.250.0)**: the compositor's Threshold pass computes the grey
//!   with the same `fma` order and compares it with the same `f32` edge, so
//!   a grey exactly on the edge is decided identically on both sides for the
//!   same input texel; only a backdrop texel whose un-premultiplied `f16`
//!   value differs (translucent backdrops) can flip.
//! - **Never an identity**; the input is clamped to `[0, 1]` (`NaN` reads
//!   as `0`) and the output is exactly `0.0` or `1.0`.

use aurora_core::ThresholdParams;

/// Photoshop's grey weights (red, green, blue).
pub const LUMA: [f32; 3] = [0.30, 0.59, 0.11];

/// The grey, on the 0 to 1 scale, at which a pixel turns white for
/// `level` (on the 0 to 255 scale).
#[must_use]
pub fn edge(level: u8) -> f32 {
    (f32::from(level) - 0.5) / 255.0
}

/// How many `f32`s [`ThresholdOp::packed_for_gpu`] returns (0.250.0).
pub const THRESHOLD_PACKED_LEN: usize = 4;

/// A Threshold adjustment ready to apply to pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThresholdOp {
    edge: f32,
}

impl ThresholdOp {
    /// Prepares `params` for application.
    #[must_use]
    pub fn new(params: &ThresholdParams) -> Self {
        Self {
            edge: edge(params.level()),
        }
    }

    /// Always `false`: every level changes some colours.
    #[must_use]
    pub const fn is_identity(&self) -> bool {
        false
    }

    /// The settings flattened for the GPU Threshold pass (0.250.0,
    /// `aurora_render::TileCompositor::composite_threshold_with_opacity`,
    /// `shaders/threshold.wgsl`): `[edge, LUMA red, LUMA green, LUMA
    /// blue]`, the very `f32`s [`Self::apply`] compares and weighs with.
    #[must_use]
    pub const fn packed_for_gpu(&self) -> [f32; THRESHOLD_PACKED_LEN] {
        [self.edge, LUMA[0], LUMA[1], LUMA[2]]
    }

    /// The black or white of one straight, encoded RGB pixel.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let [r, g, b] = rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        let grey = LUMA[0].mul_add(r, LUMA[1].mul_add(g, LUMA[2] * b));
        let value = if grey >= self.edge { 1.0 } else { 0.0 };
        [value; 3]
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::indexing_slicing
)]
mod tests {
    use super::{LUMA, ThresholdOp, edge};
    use aurora_core::ThresholdParams;

    fn op(level: u8) -> ThresholdOp {
        match ThresholdParams::new(level) {
            Ok(p) => ThresholdOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// An independent f64 model of the module docs.
    fn model(level: u8, rgb: [f32; 3]) -> f64 {
        let c = |v: f32| f64::from(v).clamp(0.0, 1.0);
        let grey = 0.30 * c(rgb[0]) + 0.59 * c(rgb[1]) + 0.11 * c(rgb[2]);
        if (grey * 255.0).round() >= f64::from(level) {
            1.0
        } else {
            0.0
        }
    }

    #[test]
    fn the_weights_are_photoshops_and_sum_to_one() {
        assert_eq!(LUMA, [0.30, 0.59, 0.11]);
        assert!((LUMA.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_edge_is_half_a_level_below_the_level() {
        assert!((edge(128) - 127.5 / 255.0).abs() < 1e-7);
        assert!((edge(1) - 0.5 / 255.0).abs() < 1e-7);
        assert!((edge(255) - 254.5 / 255.0).abs() < 1e-7);
    }

    #[test]
    fn black_and_white_are_fixed_for_every_level_and_the_output_is_binary() {
        for level in [1_u8, 2, 60, 128, 232, 254, 255] {
            let t = op(level);
            assert_eq!(t.apply([0.0; 3]), [0.0; 3], "{level}");
            assert_eq!(t.apply([1.0; 3]), [1.0; 3], "{level}");
            let out = t.apply([0.3, 0.6, 0.9]);
            assert!(out[0] == out[1] && out[1] == out[2] && (out[0] == 0.0 || out[0] == 1.0));
        }
        assert!(!op(128).is_identity());
    }

    #[test]
    fn each_channel_has_its_own_weight() {
        // Level 25: the grey must reach 24.5 of 255, i.e. 0.0961.
        let t = op(25);
        assert_eq!(
            t.apply([0.33, 0.0, 0.0]),
            [1.0; 3],
            "red 0.30 * 0.33 = 0.099"
        );
        assert_eq!(
            t.apply([0.31, 0.0, 0.0]),
            [0.0; 3],
            "red 0.30 * 0.31 = 0.093"
        );
        assert_eq!(
            t.apply([0.0, 0.17, 0.0])[0],
            1.0,
            "green 0.59 * 0.17 = 0.100"
        );
        assert_eq!(
            t.apply([0.0, 0.16, 0.0])[0],
            0.0,
            "green 0.59 * 0.16 = 0.094"
        );
        assert_eq!(t.apply([0.0, 0.0, 1.0])[0], 1.0, "blue 0.11 * 1 = 0.11");
        assert_eq!(op(29).apply([0.0, 0.0, 1.0])[0], 0.0, "0.11 < 28.5 / 255");
        assert_eq!(op(28).apply([0.0, 0.0, 1.0])[0], 1.0, "0.11 >= 27.5 / 255");
    }

    #[test]
    fn a_rec709_grey_would_decide_differently_where_the_weights_differ() {
        // Pure green: Photoshop 0.59, Rec.709 0.7152. Level 160 (edge 0.6255)
        // keeps this green black; Rec.709 would make it white.
        assert_eq!(op(160).apply([0.0, 1.0, 0.0]), [0.0; 3]);
        assert_eq!(op(150).apply([0.0, 1.0, 0.0]), [1.0; 3]);
    }

    #[test]
    fn it_matches_the_model_over_a_colour_grid_at_several_levels() {
        let steps: Vec<f32> = (0..=10_u8).map(|i| f32::from(i) / 10.0).collect();
        for level in [1_u8, 37, 60, 115, 128, 172, 232, 255] {
            let t = op(level);
            for &r in &steps {
                for &g in &steps {
                    for &b in &steps {
                        let got = t.apply([r, g, b])[0];
                        let want = model(level, [r, g, b]) as f32;
                        // Skip values within the f32 noise of the edge.
                        let grey = f64::from(LUMA[0] * r + LUMA[1] * g + LUMA[2] * b) * 255.0;
                        if (grey - (f64::from(level) - 0.5)).abs() < 1e-3 {
                            continue;
                        }
                        assert_eq!(got, want, "level {level} at {r} {g} {b}");
                    }
                }
            }
        }
    }

    #[test]
    fn out_of_gamut_values_are_clamped_and_nan_reads_as_black() {
        let t = op(128);
        assert_eq!(t.apply([9.0, 9.0, 9.0]), [1.0; 3]);
        assert_eq!(t.apply([-9.0, -9.0, -9.0]), [0.0; 3]);
        assert_eq!(t.apply([f32::NAN; 3]), [0.0; 3]);
        // Clamping happens per channel before the weights: 9 -> 1.
        assert_eq!(op(77).apply([9.0, -9.0, -9.0]), [1.0; 3]);
        assert_eq!(op(78).apply([9.0, -9.0, -9.0]), [0.0; 3]);
    }
}
