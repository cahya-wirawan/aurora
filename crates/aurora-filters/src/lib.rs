//! Filter and adjustment node implementations.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! As of 0.155.0 this crate holds one real adjustment, Curves
//! ([`curves`]); Levels ([`levels`]) joined in 0.202.0 and Hue/Saturation
//! ([`hue_saturation`]) in 0.203.0; Vibrance ([`vibrance`]), Exposure
//! ([`exposure`]) and Photo Filter ([`photo_filter`]) joined in 0.239.0
//! (CPU only); Black & White ([`black_white`]) and Color Balance
//! ([`color_balance`]) joined in 0.240.0 (CPU only); every other filter and
//! adjustment is still to come.

pub mod black_white;
pub mod brightness_contrast;
pub mod channel_mixer;
pub mod color_balance;
pub mod curves;
pub mod exposure;
pub mod gradient_map;
pub mod hue_saturation;
pub mod invert;
pub mod levels;
pub mod photo_filter;
pub mod posterize;
pub mod selective_color;
pub mod shadow;
pub mod stroke;
pub mod threshold;
pub mod vibrance;

pub use black_white::{BLACK_WHITE_PACKED_LEN, BlackWhiteOp};
pub use brightness_contrast::BrightnessContrastOp;
pub use channel_mixer::ChannelMixerOp;
pub use color_balance::{COLOR_BALANCE_PACKED_LEN, ColorBalanceOp, SHIFT_AT_100, range_weights};
pub use curves::{CURVES_LUT_INTERVALS, CURVES_PACKED_HEADER, CURVES_PACKED_LEN, CurvesLut};
pub use exposure::{EXPOSURE_PACKED_LEN, ExposureOp};
pub use gradient_map::GradientMapOp;
pub use hue_saturation::{HUE_SATURATION_PACKED_LEN, HueSaturationOp, range_weight};
pub use invert::InvertOp;
pub use levels::{EXACT_INTERVALS, LEVELS_LUT_INTERVALS, LEVELS_PACKED_LEN, LevelsLut};
pub use photo_filter::{PHOTO_FILTER_PACKED_LEN, PhotoFilterOp};
pub use posterize::PosterizeOp;
pub use selective_color::SelectiveColorOp;
pub use shadow::shadow_coverage;
pub use stroke::{StrokeSide, stroke_coverage, stroke_reach};
pub use threshold::ThresholdOp;
pub use vibrance::{VIBRANCE_PACKED_LEN, VibranceOp};

/// The crate's own name, kept from the skeleton so CI's crate-name check
/// still has something to check.
#[must_use]
pub const fn crate_name() -> &'static str {
    "aurora-filters"
}

#[cfg(test)]
mod tests {
    use super::crate_name;

    /// Per-pixel cost of the 0.239.0 operations over a 4096 x 4096 layer
    /// (16.8 Mpx) on pseudo-random pixels, in whatever profile the tests
    /// run (dev). Not an assertion: run with
    /// `cargo test -p aurora-filters --lib cost_probe -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement, prints timings"]
    #[allow(
        clippy::cast_precision_loss,
        clippy::print_stderr,
        clippy::too_many_lines
    )]
    fn cost_probe_for_the_0_239_and_0_240_adjustments() {
        use super::{ExposureOp, PhotoFilterOp, VibranceOp};
        let count = 4096_usize * 4096;
        let mut state = 0x9E37_79B9_u32;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / 16_777_216.0
        };
        let pixels: Vec<[f32; 3]> = (0..count).map(|_| [next(), next(), next()]).collect();
        let vibrance = match aurora_core::VibranceParams::new(60.0, 20.0) {
            Ok(p) => VibranceOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        };
        let exposure = match aurora_core::ExposureParams::new(0.7, 0.02, 1.4) {
            Ok(p) => ExposureOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        };
        let filter = PhotoFilterOp::new(&aurora_core::PhotoFilterParams::default());
        let time = |name: &str, f: &dyn Fn([f32; 3]) -> [f32; 3]| {
            let start = std::time::Instant::now();
            let mut sink = 0.0_f32;
            for &p in &pixels {
                sink += f(p)[1];
            }
            let elapsed = start.elapsed();
            eprintln!(
                "{name}: {:.3} s for {count} px = {:.1} ns/px (checksum {sink})",
                elapsed.as_secs_f64(),
                elapsed.as_secs_f64() * 1e9 / count as f64
            );
        };
        time("vibrance", &|p| vibrance.apply(p));
        time("exposure", &|p| exposure.apply(p));
        time("photo filter", &|p| filter.apply(p));
        let hue = super::HueSaturationOp::new(&aurora_core::HueSaturationParams::identity());
        let hue =
            match aurora_core::HueSaturationParams::new(20.0, 30.0, 10.0, false, 0.0, 25.0, 0.0) {
                Ok(p) => super::HueSaturationOp::new(&p),
                Err(_) => hue,
            };
        time("hue/saturation (reference)", &|p| hue.apply(p));
        // 0.240.0: Black & White (plain and tinted) and Color Balance.
        let bw = super::BlackWhiteOp::new(&aurora_core::BlackWhiteParams::default());
        time("black & white", &|p| bw.apply(p));
        let tinted = match aurora_core::BlackWhiteParams::new(
            aurora_core::BLACK_WHITE_DEFAULTS,
            true,
            35.0,
            25.0,
        ) {
            Ok(p) => super::BlackWhiteOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        };
        time("black & white, tinted", &|p| tinted.apply(p));
        let balance = match aurora_core::ColorBalanceParams::new(
            [[20.0, -10.0, 5.0], [30.0, 15.0, -25.0], [-10.0, 5.0, 10.0]],
            true,
        ) {
            Ok(p) => super::ColorBalanceOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        };
        time("color balance", &|p| balance.apply(p));
        // 0.244.0: Invert, Threshold, Posterize, Brightness/Contrast.
        let invert = super::InvertOp::new();
        time("invert", &|p| invert.apply(p));
        let threshold = match aurora_core::ThresholdParams::new(128) {
            Ok(p) => super::ThresholdOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        };
        time("threshold", &|p| threshold.apply(p));
        let posterize = match aurora_core::PosterizeParams::new(4) {
            Ok(p) => super::PosterizeOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        };
        time("posterize", &|p| posterize.apply(p));
        for (b, c, legacy, name) in [
            (20, 57, true, "brightness/contrast, legacy"),
            (23, 70, false, "brightness/contrast, default"),
        ] {
            let op = match aurora_core::BrightnessContrastParams::new(b, c, legacy) {
                Ok(p) => super::BrightnessContrastOp::new(&p),
                Err(err) => unreachable!("{err:?}"),
            };
            time(name, &|p| op.apply(p));
        }
        // 0.245.0: Channel Mixer (colour and Monochrome) and Selective Color
        // (Relative and Absolute).
        for (mono, name) in [
            (false, "channel mixer"),
            (true, "channel mixer, monochrome"),
        ] {
            let op = match aurora_core::ChannelMixerParams::new(
                [
                    [30.0, 59.0, 11.0, 5.0],
                    [-20.0, 120.0, 0.0, 0.0],
                    [10.0, 10.0, 80.0, -5.0],
                ],
                mono,
            ) {
                Ok(p) => super::ChannelMixerOp::new(&p),
                Err(err) => unreachable!("{err:?}"),
            };
            time(name, &|p| op.apply(p));
        }
        let mut ranges = [[0.0_f32; 4]; 9];
        for (i, row) in ranges.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (((i * 7 + j * 13) % 41) as f32 - 20.0) * 3.0;
            }
        }
        for (absolute, name) in [
            (false, "selective color, relative"),
            (true, "selective color, absolute"),
        ] {
            let op = match aurora_core::SelectiveColorParams::new(ranges, absolute) {
                Ok(p) => super::SelectiveColorOp::new(&p),
                Err(err) => unreachable!("{err:?}"),
            };
            time(name, &|p| op.apply(p));
        }
    }

    #[test]
    fn reports_its_name() {
        assert_eq!(crate_name(), "aurora-filters");
    }
}
