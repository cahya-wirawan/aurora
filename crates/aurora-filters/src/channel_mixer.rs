//! The Channel Mixer adjustment's pixel math (0.245.0).
//!
//! [`ChannelMixerOp`] applies an [`aurora_core::ChannelMixerParams`] to
//! **straight (un-premultiplied) `f32` RGB**, encoded sRGB in and out,
//! alpha never an input or output. The formula is on the parameter type
//! (`aurora-core`'s `channel_mixer` module).
//!
//! # Decisions, with reasons
//!
//! - **Encoded sRGB, not linear light.** Photoshop's mixer works on the
//!   document's own (gamma-encoded) values; Aurora's documents are sRGB.
//!   **UNVALIDATED against Photoshop**: every fixture holds the neutral
//!   mixer, so only the neutral case is known to match (it is exact here).
//! - **Input clamped to `[0, 1]`** (`NaN` reads as `0`), output clamped to
//!   `[0, 1]`: a mix over 100 % clips like Photoshop's 8-bit layers do.
//! - **Monochrome uses the first row** for the grey (which row Photoshop
//!   stores it in is unverified; the other two rows are ignored).
//! - **The identity is returned bit for bit**: a neutral mixer is skipped by
//!   the caller ([`ChannelMixerOp::is_identity`]), and `apply` of the
//!   identity rows returns the clamped input exactly.

use aurora_core::ChannelMixerParams;

/// A Channel Mixer adjustment ready to apply to pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelMixerOp {
    /// Source weights as fractions (`percent / 100`) and the constant as a
    /// fraction, per output.
    rows: [[f32; 4]; 3],
    monochrome: bool,
    identity: bool,
}

impl ChannelMixerOp {
    /// Prepares `params` for application.
    #[must_use]
    pub fn new(params: &ChannelMixerParams) -> Self {
        Self {
            rows: params.rows().map(|row| row.map(|v| v / 100.0)),
            monochrome: params.monochrome(),
            identity: params.is_identity(),
        }
    }

    /// Whether the mixer changes nothing.
    #[must_use]
    pub const fn is_identity(&self) -> bool {
        self.identity
    }

    /// The mixed colour of one straight, encoded RGB pixel.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let [r, g, b] = rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        let mix = |row: &[f32; 4]| {
            let [wr, wg, wb, k] = *row;
            wr.mul_add(r, wg.mul_add(g, wb.mul_add(b, k)))
                .clamp(0.0, 1.0)
        };
        let [red, green, blue] = &self.rows;
        if self.monochrome {
            return [mix(red); 3];
        }
        [mix(red), mix(green), mix(blue)]
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::indexing_slicing)]
mod tests {
    use super::ChannelMixerOp;
    use aurora_core::ChannelMixerParams;

    fn params(rows: [[f32; 4]; 3], mono: bool) -> ChannelMixerParams {
        match ChannelMixerParams::new(rows, mono) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn op(rows: [[f32; 4]; 3], mono: bool) -> ChannelMixerOp {
        ChannelMixerOp::new(&params(rows, mono))
    }

    /// An independent f64 model of the module docs.
    fn model(rows: [[f32; 4]; 3], mono: bool, rgb: [f32; 3]) -> [f64; 3] {
        let c = |v: f32| f64::from(v).clamp(0.0, 1.0);
        let (r, g, b) = (c(rgb[0]), c(rgb[1]), c(rgb[2]));
        let mix = |row: [f32; 4]| {
            ((f64::from(row[0]) * r + f64::from(row[1]) * g + f64::from(row[2]) * b) / 100.0
                + f64::from(row[3]) / 100.0)
                .clamp(0.0, 1.0)
        };
        if mono {
            [mix(rows[0]); 3]
        } else {
            [mix(rows[0]), mix(rows[1]), mix(rows[2])]
        }
    }

    fn grid() -> Vec<[f32; 3]> {
        let s: Vec<f32> = (0..=4_u8).map(|i| f32::from(i) / 4.0).collect();
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

    #[test]
    fn the_identity_returns_the_input_bit_for_bit() {
        let id = ChannelMixerOp::new(&ChannelMixerParams::identity());
        assert!(id.is_identity());
        for px in grid() {
            assert_eq!(id.apply(px), px);
        }
        let third = 1.0_f32 / 3.0;
        assert_eq!(id.apply([third, 0.1, 0.7]), [third, 0.1, 0.7]);
        assert!(!op(ChannelMixerParams::identity().rows(), true).is_identity());
    }

    #[test]
    fn it_matches_an_independent_model_over_a_colour_grid() {
        let sets = [
            [
                [100.0, 0.0, 0.0, 0.0],
                [0.0, 100.0, 0.0, 0.0],
                [0.0, 0.0, 100.0, 0.0],
            ],
            [
                [0.0, 0.0, 100.0, 0.0],
                [0.0, 100.0, 0.0, 0.0],
                [100.0, 0.0, 0.0, 0.0],
            ],
            [
                [30.0, 59.0, 11.0, 0.0],
                [-50.0, 150.0, 20.0, 10.0],
                [80.0, -80.0, 100.0, -20.0],
            ],
            [
                [200.0, -200.0, 50.0, 100.0],
                [-200.0, 200.0, 0.0, -100.0],
                [0.0, 0.0, 0.0, 25.0],
            ],
        ];
        for rows in sets {
            for mono in [false, true] {
                let o = op(rows, mono);
                for px in grid() {
                    let got = o.apply(px);
                    let want = model(rows, mono, px);
                    for ch in 0..3 {
                        assert!(
                            (f64::from(got[ch]) - want[ch]).abs() < 1e-6,
                            "{rows:?} mono {mono} at {px:?} channel {ch}: {} vs {}",
                            got[ch],
                            want[ch]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn each_output_uses_its_own_row_and_each_source_its_own_weight() {
        // Distinct weights everywhere: a swapped row or column changes it.
        let rows = [
            [10.0, 20.0, 30.0, 0.0],
            [40.0, 50.0, 60.0, 0.0],
            [70.0, 80.0, 90.0, 0.0],
        ];
        let o = op(rows, false);
        let out = o.apply([1.0, 0.0, 0.0]);
        assert!((out[0] - 0.10).abs() < 1e-6 && (out[1] - 0.40).abs() < 1e-6);
        assert!((out[2] - 0.70).abs() < 1e-6);
        let out = o.apply([0.0, 1.0, 0.0]);
        assert!((out[0] - 0.20).abs() < 1e-6 && (out[1] - 0.50).abs() < 1e-6);
        assert!((out[2] - 0.80).abs() < 1e-6);
        let out = o.apply([0.0, 0.0, 1.0]);
        assert!((out[0] - 0.30).abs() < 1e-6 && (out[1] - 0.60).abs() < 1e-6);
        assert!((out[2] - 0.90).abs() < 1e-6);
    }

    #[test]
    fn the_constant_is_a_percent_of_full_scale_added_after_the_mix() {
        let rows = [
            [100.0, 0.0, 0.0, 25.0],
            [0.0, 100.0, 0.0, -25.0],
            [0.0, 0.0, 0.0, 100.0],
        ];
        let o = op(rows, false);
        let out = o.apply([0.5, 0.5, 0.0]);
        assert!((out[0] - 0.75).abs() < 1e-6);
        assert!((out[1] - 0.25).abs() < 1e-6);
        assert_eq!(out[2], 1.0, "constant 100 % alone is full scale");
        assert_eq!(o.apply([0.0; 3])[0], 0.25, "a constant moves black");
    }

    #[test]
    fn monochrome_writes_one_grey_from_the_first_row_only() {
        let rows = [
            [30.0, 59.0, 11.0, 0.0],
            [100.0, 100.0, 100.0, 0.0],
            [-50.0, -50.0, -50.0, 50.0],
        ];
        let o = op(rows, true);
        for px in grid() {
            let out = o.apply(px);
            assert!(out[0] == out[1] && out[1] == out[2], "a grey at {px:?}");
            let want =
                (0.30 * f64::from(px[0]) + 0.59 * f64::from(px[1]) + 0.11 * f64::from(px[2]))
                    .clamp(0.0, 1.0);
            assert!((f64::from(out[0]) - want).abs() < 1e-6);
        }
        // Switching it off brings the other rows back.
        let colour = op(rows, false).apply([0.2, 0.4, 0.6]);
        assert_ne!(colour[0], colour[1]);
    }

    #[test]
    fn the_result_clips_and_out_of_range_input_is_clamped_first() {
        let o = op([[200.0, 200.0, 200.0, 100.0]; 3], false);
        assert_eq!(o.apply([1.0; 3]), [1.0; 3]);
        let o = op([[-200.0, -200.0, -200.0, -100.0]; 3], false);
        assert_eq!(o.apply([1.0; 3]), [0.0; 3]);
        // Input 9 is 1 and -9 is 0 before the weights apply; NaN is 0.
        let o = op(
            [
                [50.0, 0.0, 0.0, 0.0],
                [0.0, 50.0, 0.0, 0.0],
                [0.0, 0.0, 50.0, 0.0],
            ],
            false,
        );
        assert_eq!(o.apply([9.0, -9.0, f32::NAN]), [0.5, 0.0, 0.0]);
    }
}
