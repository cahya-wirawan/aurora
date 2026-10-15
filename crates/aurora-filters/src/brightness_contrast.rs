//! The Brightness/Contrast adjustment's pixel math (0.244.0).
//!
//! [`BrightnessContrastOp`] applies an
//! [`aurora_core::BrightnessContrastParams`] to **straight (un-premultiplied)
//! `f32` RGB**, encoded sRGB in and out, alpha never an input or output, no
//! 8-bit intermediate. Both modes are a per-channel function of the encoded
//! value (the fixtures' output depends on the input channel alone: zero
//! ambiguous inputs over 230 to 240 distinct input levels per layer).
//!
//! # Use Legacy (`adjustments/brightnesscontrast_legacy_rgb.psd`)
//!
//! On the 0 to 255 scale, with the mean `m` = 127
//! ([`aurora_core::BRIGHTNESS_CONTRAST_MEAN`]):
//!
//! ```text
//! out = (in + brightness - m) * slope(contrast) + m        clamped to 0..255
//! slope(c) = 1 / (1 - c / 100)  for c >= 0   (c = 100: a step at m)
//!          = 1 + c / 100        for c <  0
//! ```
//!
//! **Measured:** the fixture (brightness 20, contrast 57) is a straight
//! line, slope 2.3185 to 2.3190, pivot 127, shift 20; the formula's slope
//! at contrast 57 is 2.3256 (20 of the 246 table entries differ, by one
//! level at most; mean error 0.2 levels). **Unverified:** every other
//! contrast, the negative-contrast slope and the contrast-100 step are a
//! choice (one fixture), as is how the brightness shift combines with the
//! contrast (fitted at one point). Brightness is added in levels, not
//! scaled.
//!
//! # The default mode, an APPROXIMATION
//!
//! Photoshop's own curve is unpublished. The four fixtures
//! (`brightnesscontrast_rgb.psd`: brightness/contrast -95/-50, -31/47, 23/70,
//! 150/-21) show a smooth tone curve through (0, 0) and (255, 255), fitted
//! here as a natural cubic spline ([`aurora_core::ToneCurve`], the Curves
//! layer's) through five points whose three inner ones move by
//!
//! ```text
//! displacement(x) = BRIGHTNESS[x] * brightness / 150 + CONTRAST[x] * contrast / 100
//! ```
//!
//! at x = 64, 128 and 192 (levels; [`BRIGHTNESS`] is a bump, [`CONTRAST`] an
//! S). The six numbers are a least-squares fit to the four tables.
//! **Error against Photoshop's merged image: the whole of
//! `brightnesscontrast_rgb.psd` has a mean absolute error of 8.75 levels
//! per channel (max 52; 37.45 with the layers left out), measured by
//! `aurora-app`'s `the_four_adjustments_match_photoshops_merged_images_in_the_corpus`.
//! The four fixture layers' own tone curves are 5 to 13 levels off on
//! average.** This is NOT within 3 levels. The first fixture (-95/-50) is the worst: its
//! curve turns sharply up in the last 20 levels, which no smooth model
//! here follows. Four samples cannot support a richer model without
//! overfitting; the importer reports that the adjustment is approximate.
//!
//! # Decisions, with reasons
//!
//! - **Both sliders at zero is an exact passthrough** even out of gamut
//!   ([`BrightnessContrastOp::is_identity`], either mode); a non-identity
//!   op clamps its input to `[0, 1]` (`NaN` reads as `0`) and returns
//!   values in `[0, 1]`.
//! - **GPU (0.250.0)**: the Use Legacy line is its own pass
//!   ([`BrightnessContrastOp::packed_for_gpu`]); the default mode's table is
//!   uploaded by the compositor's Curves pass
//!   ([`BrightnessContrastOp::curve_for_gpu`]), so the GPU interpolates the
//!   very 16,385 samples the CPU does and no spline is evaluated on the GPU.
//! - **The default mode reuses [`crate::CurvesLut`]** (a 16,385-sample
//!   table with linear interpolation), so it cannot be sharper than a
//!   Curves layer; building it costs one table per parameter set, cached
//!   by the app per thread.

use crate::CurvesLut;
use aurora_core::{
    BRIGHTNESS_CONTRAST_MEAN, BrightnessContrastParams, CurvePoint, CurvesParams, ToneCurve,
};

/// How many `f32`s [`BrightnessContrastOp::packed_for_gpu`] returns
/// (0.250.0).
pub const BRIGHTNESS_CONTRAST_PACKED_LEN: usize = 4;

/// The default mode's brightness displacement of the inner points at
/// x = 64, 128, 192, in levels, at brightness 150.
pub const BRIGHTNESS: [f32; 3] = [80.0, 100.0, 80.0];

/// The default mode's contrast displacement of the inner points, in
/// levels, at contrast 100.
pub const CONTRAST: [f32; 3] = [-35.0, -7.0, 33.0];

/// The x of the inner points, in levels.
pub const KNOTS: [f32; 3] = [64.0, 128.0, 192.0];

/// The legacy slope for a contrast slider value in `-100..=100`
/// (`f32::INFINITY` at 100: a step).
#[must_use]
pub fn legacy_slope(contrast: i16) -> f32 {
    let c = f32::from(contrast);
    if contrast >= 100 {
        f32::INFINITY
    } else if contrast >= 0 {
        1.0 / (1.0 - c / 100.0)
    } else {
        1.0 + c / 100.0
    }
}

/// The inner points of the default mode's curve for the sliders, as
/// `(x, y)` on the 0 to 1 scale.
#[must_use]
pub fn default_points(brightness: i16, contrast: i16) -> [(f32, f32); 3] {
    let b = f32::from(brightness) / 150.0;
    let c = f32::from(contrast) / 100.0;
    [0, 1, 2].map(|i| {
        let x = KNOTS.get(i).copied().unwrap_or(0.0);
        let d = BRIGHTNESS.get(i).copied().unwrap_or(0.0) * b
            + CONTRAST.get(i).copied().unwrap_or(0.0) * c;
        (x / 255.0, ((x + d) / 255.0).clamp(0.01, 0.99))
    })
}

/// How a Brightness/Contrast layer maps a channel.
#[derive(Debug, Clone, PartialEq)]
enum Mode {
    /// Not applied.
    Identity,
    /// Linear: `(in + shift - mean) * slope + mean` on the 0 to 1 scale.
    Legacy { shift: f32, mean: f32, slope: f32 },
    /// The fitted tone curve, with the parameters it was built from (the
    /// GPU Curves pass caches its table upload per parameter set).
    Curve(Box<CurvesLut>, Box<CurvesParams>),
}

/// A Brightness/Contrast adjustment ready to apply to pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct BrightnessContrastOp {
    mode: Mode,
}

impl BrightnessContrastOp {
    /// Prepares `params` for application.
    #[must_use]
    pub fn new(params: &BrightnessContrastParams) -> Self {
        if params.is_identity() {
            return Self {
                mode: Mode::Identity,
            };
        }
        if params.legacy() {
            return Self {
                mode: Mode::Legacy {
                    shift: f32::from(params.brightness()) / 255.0,
                    mean: f32::from(BRIGHTNESS_CONTRAST_MEAN) / 255.0,
                    slope: legacy_slope(params.contrast()),
                },
            };
        }
        let [(x1, y1), (x2, y2), (x3, y3)] = default_points(params.brightness(), params.contrast());
        let points = [
            CurvePoint::new(0.0, 0.0),
            CurvePoint::new(x1, y1),
            CurvePoint::new(x2, y2),
            CurvePoint::new(x3, y3),
            CurvePoint::new(1.0, 1.0),
        ];
        let curve = match ToneCurve::new(&points) {
            Ok(composite) => CurvesParams {
                composite,
                red: None,
                green: None,
                blue: None,
            },
            // Unreachable: the points are finite, ordered and 64 levels
            // apart. The identity is the calm fallback.
            Err(_) => CurvesParams::identity(),
        };
        let lut = CurvesLut::new(&curve);
        Self {
            mode: Mode::Curve(Box::new(lut), Box::new(curve)),
        }
    }

    /// Whether [`Self::apply`] returns its input unchanged.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        match &self.mode {
            Mode::Identity => true,
            Mode::Legacy { .. } => false,
            Mode::Curve(lut, _) => lut.is_identity(),
        }
    }

    /// The settings of the two linear cases flattened for the GPU
    /// Brightness/Contrast pass (0.250.0,
    /// `aurora_render::TileCompositor::composite_brightness_contrast_with_opacity`,
    /// `shaders/brightness_contrast.wgsl`): `[kind, shift, mean, slope]`,
    /// `kind` 0 = not applied (an exact passthrough), 1 = the Use Legacy
    /// line, 2 = the contrast-100 step at the pivot (`slope` is then 0: the
    /// CPU's infinity never reaches the GPU). `None` for the default mode,
    /// whose 16,385-sample table the Curves pass uploads
    /// ([`Self::curve_for_gpu`]).
    #[must_use]
    pub fn packed_for_gpu(&self) -> Option<[f32; BRIGHTNESS_CONTRAST_PACKED_LEN]> {
        match &self.mode {
            Mode::Identity => Some([0.0; BRIGHTNESS_CONTRAST_PACKED_LEN]),
            Mode::Legacy { shift, mean, slope } => Some(if slope.is_infinite() {
                [2.0, *shift, *mean, 0.0]
            } else {
                [1.0, *shift, *mean, *slope]
            }),
            Mode::Curve(..) => None,
        }
    }

    /// The default mode's tone curve, as the parameters it was built from
    /// and its built table: what the GPU Curves pass maps the colour
    /// through (composite curve only, so [`Self::apply`] and the Curves
    /// pass compute the same function of each channel). `None` for the
    /// identity and the legacy line.
    #[must_use]
    pub fn curve_for_gpu(&self) -> Option<(&CurvesParams, &CurvesLut)> {
        match &self.mode {
            Mode::Curve(lut, params) => Some((params, lut)),
            Mode::Identity | Mode::Legacy { .. } => None,
        }
    }

    fn channel(&self, index: usize, v: f32) -> f32 {
        match &self.mode {
            Mode::Identity => v,
            Mode::Legacy { shift, mean, slope } => {
                let x = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
                let moved = x + shift - mean;
                let out = if slope.is_infinite() {
                    // Contrast 100: a step at the pivot.
                    if moved >= 0.0 { 1.0 } else { 0.0 }
                } else {
                    moved.mul_add(*slope, *mean)
                };
                out.clamp(0.0, 1.0)
            }
            Mode::Curve(lut, _) => lut.map_channel(index, v),
        }
    }

    /// The adjusted colour of one straight, encoded RGB pixel.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        if matches!(self.mode, Mode::Identity) {
            return rgb;
        }
        let [r, g, b] = rgb;
        [self.channel(0, r), self.channel(1, g), self.channel(2, b)]
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::cast_precision_loss)]
mod tests {
    use super::{BrightnessContrastOp, KNOTS, default_points, legacy_slope};
    use aurora_core::BrightnessContrastParams;

    fn op(brightness: i16, contrast: i16, legacy: bool) -> BrightnessContrastOp {
        match BrightnessContrastParams::new(brightness, contrast, legacy) {
            Ok(p) => BrightnessContrastOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn grey(op: &BrightnessContrastOp, v: f32) -> f32 {
        op.apply([v, v, v])[1]
    }

    #[test]
    fn zero_zero_is_an_exact_passthrough_in_both_modes_even_out_of_gamut() {
        for legacy in [false, true] {
            let t = op(0, 0, legacy);
            assert!(t.is_identity());
            assert_eq!(t.apply([-2.0, 0.1234, 9.0]), [-2.0, 0.1234, 9.0]);
        }
        assert!(!op(1, 0, false).is_identity());
        assert!(!op(0, 1, false).is_identity());
        assert!(!op(0, 1, true).is_identity());
    }

    #[test]
    fn legacy_is_a_line_about_127_with_the_brightness_added_in_levels() {
        // The fixture: brightness 20, contrast 57.
        let t = op(20, 57, true);
        let slope = 1.0 / (1.0 - 0.57);
        for level in [60.0_f32, 80.0, 107.0, 127.0, 140.0, 150.0] {
            let want = ((level + 20.0 - 127.0) * slope + 127.0) / 255.0;
            assert!((grey(&t, level / 255.0) - want).abs() < 1e-5, "{level}");
        }
        // The pivot moves with the brightness: input 107 lands on 127.
        assert!((grey(&t, 107.0 / 255.0) - 127.0 / 255.0).abs() < 1e-5);
    }

    #[test]
    fn legacy_matches_the_fixtures_table_within_one_level() {
        // Measured pairs from brightnesscontrast_legacy_rgb.psd's merged image.
        let t = op(20, 57, true);
        for (x, y) in [
            (52, 0),
            (56, 9),
            (64, 27),
            (96, 101),
            (128, 176),
            (160, 250),
            (162, 255),
        ] {
            let got = grey(&t, x as f32 / 255.0) * 255.0;
            assert!((got - y as f32).abs() <= 1.0, "x {x}: {got} vs {y}");
        }
    }

    #[test]
    fn legacy_slopes_are_one_at_zero_and_the_documented_ends() {
        assert_eq!(legacy_slope(0), 1.0);
        assert!((legacy_slope(50) - 2.0).abs() < 1e-6);
        assert!((legacy_slope(-50) - 0.5).abs() < 1e-6);
        assert!(legacy_slope(100).is_infinite());
        assert_eq!(legacy_slope(-100), 0.0);
        let step = op(0, 100, true);
        assert_eq!(grey(&step, 126.0 / 255.0), 0.0);
        assert_eq!(grey(&step, 127.0 / 255.0), 1.0);
    }

    #[test]
    fn legacy_clamps_to_the_unit_range_and_negative_contrast_flattens() {
        let t = op(100, 0, true);
        assert_eq!(grey(&t, 1.0), 1.0);
        assert!((grey(&t, 0.0) - 100.0 / 255.0).abs() < 1e-5);
        let flat = op(0, -100, true);
        assert!((grey(&flat, 0.0) - 127.0 / 255.0).abs() < 1e-5);
        assert!((grey(&flat, 1.0) - 127.0 / 255.0).abs() < 1e-5);
        assert_eq!(
            op(0, 10, true).apply([f32::NAN; 3])[0],
            op(0, 10, true).apply([0.0; 3])[0]
        );
    }

    #[test]
    fn the_default_mode_pins_black_and_white_and_stays_in_range_for_every_corner() {
        for (b, c) in [
            (150, 100),
            (150, -50),
            (-150, 100),
            (-150, -50),
            (-95, -50),
            (23, 70),
        ] {
            let t = op(b, c, false);
            assert!(grey(&t, 0.0).abs() < 1e-6, "{b} {c}");
            assert!((grey(&t, 1.0) - 1.0).abs() < 1e-6, "{b} {c}");
            for i in 0..=255_u16 {
                let v = grey(&t, f32::from(i) / 255.0);
                assert!((0.0..=1.0).contains(&v), "{b} {c} {i}: {v}");
            }
        }
    }

    #[test]
    fn positive_brightness_lifts_the_middle_and_negative_lowers_it() {
        assert!(grey(&op(60, 0, false), 0.5) > 0.5 + 0.05);
        assert!(grey(&op(-60, 0, false), 0.5) < 0.5 - 0.05);
        assert!(
            grey(&op(0, 60, false), 0.9) > 0.9,
            "contrast brightens highlights"
        );
        assert!(
            grey(&op(0, 60, false), 0.1) < 0.1,
            "contrast darkens shadows"
        );
        assert!(grey(&op(0, -40, false), 0.9) < 0.9);
    }

    #[test]
    fn the_inner_points_follow_the_documented_coefficients() {
        let p = default_points(150, 0);
        assert_eq!(p.map(|(x, _)| x * 255.0), KNOTS);
        assert!((p[1].1 * 255.0 - 228.0).abs() < 1e-3, "128 + 100");
        assert!((p[0].1 * 255.0 - 144.0).abs() < 1e-3, "64 + 80");
        let q = default_points(0, 100);
        assert!((q[0].1 * 255.0 - 29.0).abs() < 1e-3, "64 - 35");
        assert!((q[2].1 * 255.0 - 225.0).abs() < 1e-3, "192 + 33");
        assert!((q[1].1 * 255.0 - 121.0).abs() < 1e-3, "128 - 7");
    }

    #[test]
    fn the_channels_are_independent_in_both_modes() {
        for t in [op(40, 30, false), op(40, 30, true)] {
            let out = t.apply([0.2, 0.5, 0.8]);
            assert_eq!(out[0], grey(&t, 0.2));
            assert_eq!(out[1], grey(&t, 0.5));
            assert_eq!(out[2], grey(&t, 0.8));
        }
    }
}
