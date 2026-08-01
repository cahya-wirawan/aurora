//! sRGB transfer-function conversion (linear light <-> gamma-encoded).
//!
//! Aurora's own working-space policy — which colour space filters and
//! blending actually compute in, and whether it's always linear sRGB or
//! adapts to a document's own gamut — has no concrete consumer yet
//! (`aurora-filters` and `aurora-render`'s own colour wiring don't exist
//! yet), so it's deliberately not designed speculatively here, same
//! "primitive built, real consumer decides the policy later" shape
//! `aurora_graph::RenderGraph<N>` and `aurora_render::Executor` already
//! used. What's implemented: the actual transfer-function math itself,
//! which is unambiguous, well-specified (IEC 61966-2-1), and needed
//! regardless of how that policy eventually shakes out.

/// Converts a linear-light sample to sRGB's gamma-encoded representation
/// (IEC 61966-2-1's actual piecewise curve, not a plain 2.2 gamma
/// approximation).
///
/// Sign-preserving: negative inputs are encoded via their magnitude and
/// the sign reapplied, rather than raising a negative number to a
/// fractional power (`NaN` in IEEE arithmetic). This matters for Aurora
/// specifically — invariant §7.3.1b requires HDR/scene-referred values
/// (negative, or above `1.0`) to survive rather than being clamped, the
/// same property `crate::Transform` already verifies for ICC transforms
/// end to end.
#[must_use]
pub fn linear_to_srgb(linear: f32) -> f32 {
    let sign = linear.signum();
    let magnitude = linear.abs();
    let encoded = if magnitude <= 0.003_130_8 {
        magnitude * 12.92
    } else {
        1.055 * magnitude.powf(1.0 / 2.4) - 0.055
    };
    sign * encoded
}

/// The inverse of [`linear_to_srgb`]: sRGB gamma-encoded to linear light.
/// Same sign-preserving extension for negative inputs.
#[must_use]
pub fn srgb_to_linear(encoded: f32) -> f32 {
    let sign = encoded.signum();
    let magnitude = encoded.abs();
    let linear = if magnitude <= 0.04045 {
        magnitude / 12.92
    } else {
        ((magnitude + 0.055) / 1.055).powf(2.4)
    };
    sign * linear
}

#[cfg(test)]
mod tests {
    use super::{linear_to_srgb, srgb_to_linear};

    #[test]
    // Exact, bit-representable literals (0.0 * anything = 0.0), not
    // accumulated computation noise -- same reasoning `tree::tests`
    // already documents for its own float_cmp allows.
    #[allow(clippy::float_cmp)]
    fn zero_maps_to_zero_both_directions() {
        assert_eq!(linear_to_srgb(0.0), 0.0);
        assert_eq!(srgb_to_linear(0.0), 0.0);
    }

    #[test]
    fn one_maps_to_one_both_directions() {
        // Not exact equality: 1.055/0.055 aren't exactly representable in
        // f32, so `1.055 * 1.0.powf(x) - 0.055` lands a hair under 1.0
        // (0.99999994 in practice) -- real floating-point rounding, not a
        // formula bug, so an epsilon is correct here rather than
        // clippy::float_cmp's "accumulated noise" smell.
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-6);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn round_trips_within_a_small_epsilon_across_the_normal_range() {
        for i in 0u8..=20 {
            let x = f32::from(i) / 20.0;
            let round_tripped = srgb_to_linear(linear_to_srgb(x));
            assert!(
                (round_tripped - x).abs() < 1e-5,
                "linear {x} -> srgb -> linear {round_tripped}, expected ~{x}"
            );
        }
    }

    #[test]
    fn mid_gray_encodes_brighter_than_linear_mid_gray() {
        // The whole point of a gamma-encoded working space: 0.5 linear
        // encodes to well above 0.5 (perceptually closer to "middle
        // gray" on a typical display), not an identity mapping.
        let encoded = linear_to_srgb(0.5);
        assert!(
            encoded > 0.5,
            "expected sRGB(0.5) > 0.5, got {encoded} -- otherwise this isn't \
             the sRGB curve, just identity"
        );
    }

    #[test]
    fn negative_extended_range_values_survive_rather_than_becoming_nan() {
        // Invariant §7.3.1b: HDR/scene-referred values, including negative
        // ones from wide-gamut colour math, must be preserved, not
        // clamped -- and definitely not turned into NaN by raising a
        // negative number to a fractional power.
        let encoded = linear_to_srgb(-0.5);
        assert!(
            !encoded.is_nan(),
            "got NaN encoding a negative linear value"
        );
        assert!(encoded < 0.0, "sign must be preserved, got {encoded}");

        let round_tripped = srgb_to_linear(encoded);
        assert!(
            (round_tripped - (-0.5)).abs() < 1e-5,
            "-0.5 -> srgb -> linear {round_tripped}, expected ~-0.5"
        );
    }

    #[test]
    fn above_one_hdr_values_survive_rather_than_clamping() {
        let encoded = linear_to_srgb(2.0);
        assert!(!encoded.is_nan());
        assert!(
            encoded > 1.0,
            "expected an HDR value to encode above 1.0, got {encoded} -- \
             clamping to 1.0 would silently lose the superwhite value"
        );

        let round_tripped = srgb_to_linear(encoded);
        assert!(
            (round_tripped - 2.0).abs() < 1e-4,
            "2.0 -> srgb -> linear {round_tripped}, expected ~2.0"
        );
    }
}
