//! The Invert adjustment's pixel math (0.244.0): `1 - x` per channel.
//!
//! [`InvertOp`] works on **straight (un-premultiplied) `f32` RGB**, encoded
//! sRGB in and out, alpha never an input or output, no 8-bit intermediate.
//! It is Photoshop's Invert (`adjustments/invert_rgb.psd`: every masked
//! pixel is `255 - x`, 0 levels of error against Photoshop's merged image).
//!
//! The input is clamped to `[0, 1]` first (`NaN` reads as `0`), so the
//! output is in `[0, 1]`. **Invert is never an identity**: there is no
//! parameter, and applying it twice returns the clamped input.
//!
//! No GPU pass: a visible Invert layer refuses the GPU compositing path
//! (a later task ports it).

/// An Invert adjustment. It has no settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InvertOp;

impl InvertOp {
    /// The inverting adjustment.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Always `false`: Invert changes every colour but mid-grey 0.5.
    #[must_use]
    pub const fn is_identity(&self) -> bool {
        false
    }

    /// The inverted colour of one straight, encoded RGB pixel.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        rgb.map(|v| 1.0 - if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) })
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::indexing_slicing
)]
mod tests {
    use super::InvertOp;

    #[test]
    fn it_is_one_minus_the_clamped_value_and_never_an_identity() {
        let op = InvertOp::new();
        assert!(!op.is_identity());
        assert_eq!(op.apply([0.0, 1.0, 0.25]), [1.0, 0.0, 0.75]);
        assert_eq!(op.apply([0.5, 0.125, 0.875]), [0.5, 0.875, 0.125]);
    }

    #[test]
    fn out_of_gamut_and_nan_inputs_are_clamped_first() {
        let op = InvertOp::new();
        assert_eq!(op.apply([-3.0, 7.0, f32::NAN]), [1.0, 0.0, 1.0]);
        assert_eq!(
            op.apply([f32::INFINITY, f32::NEG_INFINITY, 0.5]),
            [0.0, 1.0, 0.5]
        );
    }

    #[test]
    fn applying_it_twice_returns_the_input_for_a_dyadic_value() {
        let op = InvertOp::new();
        for v in [0.0_f32, 0.0625, 0.25, 0.5, 0.75, 1.0] {
            assert_eq!(op.apply(op.apply([v, v, v])), [v, v, v]);
        }
    }
}
