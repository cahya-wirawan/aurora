//! The Posterize adjustment's pixel math (0.244.0).
//!
//! [`PosterizeOp`] applies an [`aurora_core::PosterizeParams`] to
//! **straight (un-premultiplied) `f32` RGB**, encoded sRGB in and out,
//! alpha never an input or output. The formula and its fit against the
//! fixtures are on the parameter type (`aurora-core`'s `posterize`
//! module): a channel `x` becomes `min(floor(x n), n - 1) / (n - 1)`.
//!
//! # Decisions, with reasons
//!
//! - **Bins are `[k / n, (k + 1) / n)`**, the 16-bit fixture's exactly; the
//!   8-bit fixture's edges sit at `256 k / n` of 255 (see the core docs).
//! - **255 levels is an identity** ([`PosterizeOp::is_identity`], an exact
//!   passthrough even for an out-of-gamut value); every other count clamps
//!   the input to `[0, 1]` (`NaN` reads as `0`) and returns one of `n`
//!   values in `[0, 1]`.

use aurora_core::PosterizeParams;

/// A Posterize adjustment ready to apply to pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PosterizeOp {
    levels: u8,
    /// `n` as `f32`.
    count: f32,
    /// `n - 1` as `f32`.
    top: f32,
    identity: bool,
}

impl PosterizeOp {
    /// Prepares `params` for application.
    #[must_use]
    pub fn new(params: &PosterizeParams) -> Self {
        let levels = params.levels();
        Self {
            levels,
            count: f32::from(levels),
            top: f32::from(levels) - 1.0,
            identity: params.is_identity(),
        }
    }

    /// Whether [`Self::apply`] returns its input unchanged.
    #[must_use]
    pub const fn is_identity(&self) -> bool {
        self.identity
    }

    /// The number of levels.
    #[must_use]
    pub const fn levels(&self) -> u8 {
        self.levels
    }

    /// One channel's posterised value.
    #[must_use]
    fn channel(&self, v: f32) -> f32 {
        let x = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
        (x * self.count).floor().min(self.top) / self.top
    }

    /// The posterised colour of one straight, encoded RGB pixel.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        if self.identity {
            return rgb;
        }
        rgb.map(|v| self.channel(v))
    }
}

#[cfg(test)]
mod tests {
    use super::PosterizeOp;
    use aurora_core::PosterizeParams;

    fn op(levels: u8) -> PosterizeOp {
        match PosterizeParams::new(levels) {
            Ok(p) => PosterizeOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// An independent f64 model of the module docs.
    fn model(n: u8, v: f32) -> f64 {
        let n = f64::from(n);
        let x = f64::from(v).clamp(0.0, 1.0);
        ((x * n).floor().min(n - 1.0)) / (n - 1.0)
    }

    #[test]
    fn two_levels_split_at_one_half() {
        let t = op(2);
        assert_eq!(t.apply([0.0, 0.49, 0.5]), [0.0, 0.0, 1.0]);
        assert_eq!(t.apply([1.0, 0.25, 0.75]), [1.0, 0.0, 1.0]);
    }

    #[test]
    fn three_levels_use_the_one_third_edges_and_a_half_middle() {
        let t = op(3);
        let third = 1.0_f32 / 3.0;
        assert_eq!(t.apply([0.0, 0.2, 0.3])[0..3], [0.0, 0.0, 0.0]);
        assert_eq!(t.apply([0.34, 0.5, 0.66])[0..3], [0.5, 0.5, 0.5]);
        assert_eq!(t.apply([0.67, 0.9, 1.0])[0..3], [1.0, 1.0, 1.0]);
        assert_eq!(t.apply([third + 0.001, 0.0, 0.0])[0], 0.5);
        assert_eq!(t.apply([third - 0.001, 0.0, 0.0])[0], 0.0);
    }

    #[test]
    fn one_is_clamped_into_the_top_bin_not_a_level_of_its_own() {
        for n in [2_u8, 3, 7, 13, 21, 128, 254] {
            assert_eq!(op(n).apply([1.0; 3]), [1.0; 3], "{n}");
            assert_eq!(op(n).apply([0.0; 3]), [0.0; 3], "{n}");
        }
    }

    #[test]
    fn it_matches_the_model_and_has_exactly_n_values() {
        for n in [2_u8, 3, 4, 7, 13, 21, 64, 200, 254] {
            let t = op(n);
            let mut seen = std::collections::BTreeSet::new();
            for i in 0..=1000_u32 {
                let v = i as f32 / 1000.0;
                let got = t.apply([v, v, v]);
                assert_eq!(got[0], got[1]);
                let want = model(n, v);
                // A value within f32 noise of a bin edge may fall either side.
                let scaled = f64::from(v) * f64::from(n);
                if (scaled - scaled.round()).abs() >= 1e-4 {
                    assert!((f64::from(got[0]) - want).abs() < 1e-6, "n {n} v {v}");
                }
                seen.insert(got[0].to_bits());
            }
            assert_eq!(seen.len(), usize::from(n).min(1001), "n {n}");
        }
    }

    #[test]
    fn each_channel_is_independent() {
        assert_eq!(op(4).apply([0.1, 0.5, 0.9]), [0.0, 2.0 / 3.0, 1.0]);
    }

    #[test]
    fn two_hundred_fifty_five_levels_are_an_exact_passthrough_even_out_of_gamut() {
        let t = op(255);
        assert!(t.is_identity());
        assert_eq!(t.apply([-2.0, 0.123_456, 7.0]), [-2.0, 0.123_456, 7.0]);
        assert!(!op(254).is_identity());
        assert!(!op(2).is_identity());
    }

    #[test]
    fn out_of_gamut_values_are_clamped_and_nan_reads_as_black() {
        let t = op(4);
        assert_eq!(t.apply([-1.0, 9.0, f32::NAN]), [0.0, 1.0, 0.0]);
        assert_eq!(t.levels(), 4);
    }
}
