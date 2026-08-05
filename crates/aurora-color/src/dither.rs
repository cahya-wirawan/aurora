//! The 8-bit import/export boundary (invariant §7.3.1b: "8-bit appears
//! only at import (promoted immediately) and export (quantized with
//! dithering)"). Nothing in between this crate and a real 8-bit file
//! format should ever see a `u8` sample.

/// Promotes one 8-bit sample to the normalized `[0.0, 1.0]` range used
/// everywhere else in the pipeline. The only correct way an 8-bit import
/// boundary should ever produce a float sample.
#[must_use]
pub fn promote_u8(sample: u8) -> f32 {
    f32::from(sample) / 255.0
}

/// Promotes one 16-bit sample to the normalized `[0.0, 1.0]` range —
/// the same import-boundary role [`promote_u8`] fills for 8-bit
/// samples, for formats (e.g. 16-bit-per-channel PNG) that carry real
/// precision beyond 8 bits worth preserving rather than truncating away
/// before it ever reaches this pipeline.
#[must_use]
pub fn promote_u16(sample: u16) -> f32 {
    f32::from(sample) / 65535.0
}

/// Quantizes one float sample to 8 bits *without* dithering — plain
/// rounding. Exposed for callers that genuinely want that (e.g. a
/// preview thumbnail, where banding is an acceptable, temporary
/// trade-off), but **not what a real export boundary should call** —
/// see [`dither_quantize`], which invariant §7.3.1b actually asks for.
/// Out-of-range input is clamped to `[0.0, 1.0]` first (HDR values have
/// nowhere to go once forced into 8 bits — clamping only happens *here*,
/// at the export boundary, never earlier in the pipeline).
#[must_use]
#[allow(clippy::cast_sign_loss)]
pub fn quantize_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Quantizes one float sample to 8 bits with ordered (Bayer) dithering at
/// pixel position `(x, y)` — breaks up banding in smooth gradients by
/// adding a small, deterministic, position-dependent offset before
/// rounding, rather than rounding every pixel in a gradient to the same
/// stair-stepped value. Deterministic and reproducible (same input value
/// at the same position always dithers the same way), unlike random
/// dithering — the same `(x, y)` position always contributes the same
/// offset, from the classic 8×8 Bayer threshold matrix.
///
/// Same clamping behaviour as [`quantize_u8`] for out-of-range input.
#[must_use]
pub fn dither_quantize(value: f32, x: u32, y: u32) -> u8 {
    let scaled = value.clamp(0.0, 1.0) * 255.0;
    let threshold = bayer_threshold(x, y);
    quantize_u8_unclamped(scaled + threshold)
}

#[allow(clippy::cast_sign_loss)]
fn quantize_u8_unclamped(scaled: f32) -> u8 {
    scaled.round().clamp(0.0, 255.0) as u8
}

const BAYER_SIZE: u32 = 8;

/// A value in `(-0.5, 0.5)` from the classic 8×8 Bayer ordered-dither
/// matrix at `(x, y)` (tiled — only `x % 8`/`y % 8` matter). Centred on
/// zero so adding it before rounding perturbs the quantization up or down
/// depending on position, rather than only ever rounding one direction.
fn bayer_threshold(x: u32, y: u32) -> f32 {
    let value = bayer_value(x % BAYER_SIZE, y % BAYER_SIZE, BAYER_SIZE);
    #[allow(clippy::cast_precision_loss)]
    let normalized = (value as f32 + 0.5) / (BAYER_SIZE * BAYER_SIZE) as f32;
    normalized - 0.5
}

/// The value at `(x, y)` in the `n`×`n` Bayer matrix (`n` a power of two),
/// via the standard recursive construction from the 2×2 base case
/// `[[0, 2], [3, 1]]` — computed algorithmically rather than transcribed
/// as a hardcoded 8×8 table, so correctness rests on the well-known
/// mathematical definition (verified in this module's own tests against
/// the structural property every Bayer matrix must have: every value
/// `0..n*n` appears exactly once) rather than on a table that could have
/// a copied-wrong entry.
fn bayer_value(x: u32, y: u32, n: u32) -> u32 {
    if n == 2 {
        match (x % 2, y % 2) {
            (0, 0) => 0,
            (1, 0) => 2,
            (0, 1) => 3,
            (1, 1) => 1,
            _ => unreachable!("x % 2 and y % 2 are both always 0 or 1"),
        }
    } else {
        let half = n / 2;
        let sub = bayer_value(x % half, y % half, half);
        let quadrant = match (x / half, y / half) {
            (0, 0) => 0,
            (1, 0) => 2,
            (0, 1) => 3,
            (1, 1) => 1,
            _ => unreachable!("x/half and y/half are both always 0 or 1 for x, y < n"),
        };
        4 * sub + quadrant
    }
}

#[cfg(test)]
mod tests {
    use super::{bayer_value, dither_quantize, promote_u8, promote_u16, quantize_u8};

    #[test]
    // 0/255 and 255/255 are exact, bit-representable results, not
    // accumulated computation noise -- same reasoning `tree::tests`
    // already documents for its own float_cmp allows.
    #[allow(clippy::float_cmp)]
    fn promote_maps_the_full_u8_range_onto_zero_to_one() {
        assert_eq!(promote_u8(0), 0.0);
        assert_eq!(promote_u8(255), 1.0);
        assert!((promote_u8(128) - 0.501_960_8).abs() < 1e-6);
    }

    #[test]
    // Same reasoning as the u8 case above: 0 and 65535 are exact.
    #[allow(clippy::float_cmp)]
    fn promote_u16_maps_the_full_range_onto_zero_to_one() {
        assert_eq!(promote_u16(0), 0.0);
        assert_eq!(promote_u16(65535), 1.0);
        assert!((promote_u16(32768) - 0.500_007_6).abs() < 1e-6);
    }

    #[test]
    fn quantize_round_trips_promote_for_every_u8_value() {
        for sample in 0..=255u8 {
            assert_eq!(
                quantize_u8(promote_u8(sample)),
                sample,
                "promote/quantize must round-trip exactly for {sample}"
            );
        }
    }

    #[test]
    fn quantize_clamps_out_of_range_input() {
        assert_eq!(quantize_u8(-1.0), 0);
        assert_eq!(quantize_u8(2.0), 255);
    }

    #[test]
    fn dither_quantize_clamps_out_of_range_input() {
        assert_eq!(dither_quantize(-1.0, 0, 0), 0);
        assert_eq!(dither_quantize(2.0, 0, 0), 255);
    }

    #[test]
    fn bayer_matrix_is_a_permutation_of_zero_to_n_squared_minus_one() {
        // The structural property that actually defines a Bayer matrix --
        // checked instead of trusting a transcribed table (this module's
        // own doc comment explains why).
        for n in [2, 4, 8] {
            let mut seen = vec![false; (n * n) as usize];
            for y in 0..n {
                for x in 0..n {
                    let v = bayer_value(x, y, n);
                    assert!(
                        v < n * n,
                        "bayer_value({x}, {y}, {n}) = {v} is out of range"
                    );
                    let Some(slot) = seen.get_mut(v as usize) else {
                        unreachable!("just asserted v < n * n == seen.len()");
                    };
                    assert!(!*slot, "value {v} repeated in the {n}x{n} matrix");
                    *slot = true;
                }
            }
            assert!(
                seen.iter().all(|&s| s),
                "the {n}x{n} matrix must cover every value 0..{}",
                n * n
            );
        }
    }

    #[test]
    fn bayer_matrix_matches_the_well_known_four_by_four_table() {
        // The standard, widely-published 4x4 Bayer matrix -- a known
        // reference to cross-check the recursive construction against,
        // the same "two independent things agreeing" discipline the
        // project's own spikes use, applied here against a published
        // constant instead of a second implementation.
        let expected: [[u32; 4]; 4] =
            [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        for (y, row) in expected.iter().enumerate() {
            for (x, &want) in row.iter().enumerate() {
                #[allow(clippy::cast_possible_truncation)]
                let got = bayer_value(x as u32, y as u32, 4);
                assert_eq!(got, want, "bayer_value({x}, {y}, 4)");
            }
        }
    }

    #[test]
    fn dithering_breaks_up_banding_across_a_flat_gradient_value() {
        // The whole point of dithering: the same input value, at
        // different positions, must not always quantize to the same
        // output -- otherwise this is just quantize_u8 with extra steps.
        // 127.5/255 sits exactly between two 8-bit levels, the worst case
        // for banding.
        let value = 127.5 / 255.0;
        let mut outputs = std::collections::HashSet::new();
        for y in 0..8 {
            for x in 0..8 {
                outputs.insert(dither_quantize(value, x, y));
            }
        }
        assert!(
            outputs.len() > 1,
            "expected dithering to produce more than one output value across \
             positions, got {outputs:?}"
        );
    }
}
