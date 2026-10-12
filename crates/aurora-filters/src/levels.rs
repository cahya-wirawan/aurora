//! The Levels adjustment's pixel math (0.202.0).
//!
//! [`LevelsLut`] turns an [`aurora_core::LevelsParams`] into lookup
//! tables and applies them to **straight (un-premultiplied) `f32` RGB**,
//! exactly like [`crate::CurvesLut`]: encoded values, per-channel record
//! first and then the master, alpha never an input or output, no 8-bit
//! intermediate (invariant 7.3.1b).
//!
//! # Decisions, with reasons
//!
//! - **Only the gamma curve is tabulated.** A Levels record is an affine
//!   input stage (clamped), the power `t^(1/gamma)` and an affine output
//!   stage (clamped). The affine stages are exact arithmetic here; only
//!   `t^(1/gamma)` on `t` in `[0, 1]` is sampled, at
//!   [`LEVELS_LUT_INTERVALS`]` + 1` evenly spaced positions and read back
//!   with linear interpolation. Sampling the *whole* record against `x`
//!   instead (as Curves does) would put the input clip's kink inside one
//!   table interval, an error of up to `slope * h / 4` with a slope of
//!   up to 255 (an input span of one 8-bit level) — about 4e-3, most of
//!   an 8-bit level — which is why Levels cannot simply reuse the Curves
//!   tables or its GPU pass.
//! - **Error bound.** With `p = 1 / gamma` and `h = 1 / 16384`, linear
//!   interpolation of `t^p` on the interval starting at `k * h` errs by
//!   at most `h^2 / 8 * max|f''|`, `f'' = p (p - 1) t^(p - 2)`. For
//!   `p >= 1` (gamma at most 1) `|f''| <= p (p - 1)` (at most `90`,
//!   `p = 10`), so the error is below `4.2e-8`. For `p < 1` the second
//!   derivative blows up at `0`; the first [`EXACT_INTERVALS`] intervals
//!   (`t < 16 h`, under `1e-3`) are therefore computed exactly with
//!   `powf`, and beyond them the bound is
//!   `p (1 - p) / 8 * k^(p - 2) * h^p`, at most `2.1e-5` for the worst
//!   gamma (9.99) at `k = 16` — under one 12-bit code value (`2.4e-4`).
//!   The tests measure the real maximum against
//!   [`aurora_core::LevelsChannel::map`] across the gamma range.
//! - **An identity record is skipped**, so it is an exact passthrough,
//!   even for an out-of-gamut value; a non-identity record clamps its
//!   input to `[0, 1]` (`NaN` reads as `0`), as the exact map does.
//! - **No GPU port.** `aurora-app` composites Levels layers on the CPU
//!   only (a visible Levels layer refuses the GPU path, as every
//!   adjustment but Curves does).

use aurora_core::{LevelsChannel, LevelsParams};

/// Equal intervals the gamma table divides `[0, 1]` into; the table holds
/// one more sample (64 KiB of `f32`). A power of two, so every sample
/// position is exact in `f32`.
pub const LEVELS_LUT_INTERVALS: usize = 1 << 14;

/// How many leading intervals use the exact `powf` instead of the table
/// (see the module docs' error bound).
pub const EXACT_INTERVALS: usize = 16;

/// One record ready to apply.
#[derive(Debug, Clone, PartialEq)]
struct Channel {
    record: LevelsChannel,
    /// `t^(1/gamma)` samples; `None` when gamma is exactly `1`.
    gamma_table: Option<Vec<f32>>,
}

impl Channel {
    fn new(record: LevelsChannel) -> Option<Self> {
        if record.is_identity() {
            return None;
        }
        #[allow(clippy::float_cmp)] // exactly-neutral gamma is skipped, not approximated
        let gamma_table = (record.gamma() != 1.0).then(|| {
            let power = 1.0 / record.gamma();
            #[allow(clippy::cast_precision_loss)]
            let last = LEVELS_LUT_INTERVALS as f32;
            (0..=LEVELS_LUT_INTERVALS)
                .map(|i| {
                    #[allow(clippy::cast_precision_loss)]
                    let t = i as f32 / last;
                    t.powf(power)
                })
                .collect()
        });
        Some(Self {
            record,
            gamma_table,
        })
    }

    fn gamma_of(&self, t: f32) -> f32 {
        let Some(table) = &self.gamma_table else {
            return t;
        };
        #[allow(clippy::cast_precision_loss)]
        let position = t * LEVELS_LUT_INTERVALS as f32;
        // `t` is in `[0, 1]`, so the truncation is exact and non-negative.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let index = (position as usize).min(LEVELS_LUT_INTERVALS - 1);
        if index < EXACT_INTERVALS {
            return t.powf(1.0 / self.record.gamma());
        }
        #[allow(clippy::cast_precision_loss)]
        let frac = position - index as f32;
        match (table.get(index), table.get(index + 1)) {
            (Some(&low), Some(&high)) => (high - low).mul_add(frac, low),
            // Unreachable: the table holds INTERVALS + 1 samples.
            _ => t,
        }
    }

    fn lookup(&self, x: f32) -> f32 {
        let t = self.record.normalised_input(x);
        self.record.output_of(self.gamma_of(t))
    }
}

/// A Levels adjustment ready to apply: one optional record per stage.
/// See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelsLut {
    master: Option<Channel>,
    channels: [Option<Channel>; 3],
}

impl LevelsLut {
    /// Builds the gamma table of every non-identity record of `params`
    /// (up to four tables of 64 KiB).
    #[must_use]
    pub fn new(params: &LevelsParams) -> Self {
        Self {
            master: Channel::new(params.master),
            channels: [
                Channel::new(params.red),
                Channel::new(params.green),
                Channel::new(params.blue),
            ],
        }
    }

    /// Whether applying this changes nothing (every record was identity).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.master.is_none() && self.channels.iter().all(Option::is_none)
    }

    /// Maps one straight RGB colour: each channel through its own record,
    /// then all three through the master. Alpha is the caller's.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let [r, g, b] = rgb;
        [
            self.map_channel(0, r),
            self.map_channel(1, g),
            self.map_channel(2, b),
        ]
    }

    /// Maps one value of colour channel `index` (`0` red, `1` green, `2`
    /// blue): its own record, then the master.
    #[must_use]
    pub fn map_channel(&self, index: usize, value: f32) -> f32 {
        let after_channel = match self.channels.get(index) {
            Some(Some(channel)) => channel.lookup(value),
            _ => value,
        };
        match &self.master {
            Some(master) => master.lookup(after_channel),
            None => after_channel,
        }
    }
}

impl From<&LevelsParams> for LevelsLut {
    fn from(params: &LevelsParams) -> Self {
        Self::new(params)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{LEVELS_LUT_INTERVALS, LevelsLut};
    use aurora_core::{GAMMA_MAX, GAMMA_MIN, LevelsChannel, LevelsParams};

    fn rec(ib: f32, iw: f32, g: f32, ob: f32, ow: f32) -> LevelsChannel {
        match LevelsChannel::new(ib, iw, g, ob, ow) {
            Ok(c) => c,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn identity_params_are_an_exact_passthrough_even_out_of_gamut() {
        let lut = LevelsLut::new(&LevelsParams::identity());
        assert!(lut.is_identity());
        for x in [-0.5_f32, 0.0, 0.123_456_7, 1.0, 3.5] {
            assert_eq!(lut.apply([x, x, x])[0].to_bits(), x.to_bits());
        }
    }

    /// The table is within 4e-5 of the exact map for the whole gamma range.
    #[test]
    fn lut_matches_the_exact_math_across_the_gamma_range() {
        let mut worst = 0.0_f32;
        for gamma in [GAMMA_MIN, 0.3, 0.7, 1.3, 2.2, 5.0, GAMMA_MAX] {
            let record = rec(0.07, 0.93, gamma, 0.05, 0.9);
            let params = LevelsParams {
                red: record,
                ..LevelsParams::identity()
            };
            let lut = LevelsLut::new(&params);
            for i in 0..=40_000_u32 {
                #[allow(clippy::cast_precision_loss)]
                let x = i as f32 / 40_000.0;
                let want = params.map_channel(0, x);
                let got = lut.map_channel(0, x);
                worst = worst.max((want - got).abs());
            }
        }
        assert!(worst < 4e-5, "worst error {worst}");
    }

    /// The worst case for the bound: a steep low end (gamma 9.99) sampled
    /// densely just above the exact region.
    #[test]
    fn steep_gamma_low_end_stays_within_the_bound() {
        let params = LevelsParams {
            master: rec(0.0, 1.0, GAMMA_MAX, 0.0, 1.0),
            ..LevelsParams::identity()
        };
        let lut = LevelsLut::new(&params);
        let mut worst = 0.0_f32;
        for i in 0..2_000_u32 {
            #[allow(clippy::cast_precision_loss)]
            let x = i as f32 / 2_000.0 * 0.01;
            worst = worst.max((params.map_channel(1, x) - lut.map_channel(1, x)).abs());
        }
        assert!(worst < 4e-5, "worst error {worst}");
    }

    #[test]
    fn channel_then_master_and_nan_and_range() {
        let params = LevelsParams {
            master: rec(0.0, 1.0, 1.0, 0.0, 0.5),
            red: rec(0.0, 0.5, 1.0, 0.0, 1.0),
            ..LevelsParams::identity()
        };
        let lut = LevelsLut::new(&params);
        let [r, g, b] = lut.apply([0.75, 0.75, 0.75]);
        assert!((r - 0.5).abs() < 1e-6 && (g - 0.375).abs() < 1e-6 && (b - 0.375).abs() < 1e-6);
        assert!(lut.map_channel(2, f32::NAN).abs() < 1e-6);
        assert!((lut.map_channel(2, 7.0) - 0.5).abs() < 1e-6, "clamped");
    }

    #[test]
    fn table_has_the_documented_size() {
        assert_eq!(LEVELS_LUT_INTERVALS, 16_384);
    }
}
