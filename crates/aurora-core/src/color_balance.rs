//! The parameters of a Color Balance adjustment (0.240.0): Photoshop's
//! three tonal ranges, three sliders each, and Preserve Luminosity.
//!
//! It lives in `aurora-core`, beside [`crate::HueSaturationParams`], so
//! `aurora-doc` can store it in a layer while `aurora-filters` applies it.
//!
//! # The math (applied by `aurora_filters::ColorBalanceOp`)
//!
//! On **straight, encoded** sRGB, input clamped to `[0, 1]`, with the
//! Rec.709 luma `Y` of the input. Each tonal range has a weight from `Y`
//! (the Dodge and Burn tools' range weights):
//!
//! ```text
//! shadows    (1 - Y)^2
//! midtones   4 Y (1 - Y)
//! highlights Y^2
//! ```
//!
//! A range's three sliders `(cyan-red, magenta-green, yellow-blue)`, each
//! `-100..=100`, move the red, green and blue channel (positive is toward
//! red, green and blue; negative toward cyan, magenta and yellow). The
//! channel `c` becomes
//!
//! ```text
//! out_c = in_c + SHIFT_AT_100 * sum over ranges of weight(range) * slider(range, c) / 100
//! ```
//!
//! with `SHIFT_AT_100 = 0.5`: a slider at 100 in the range that has full
//! weight moves its channel by half the scale. With **Preserve Luminosity**
//! the result then gets the one additive shift that restores the input's
//! luma (`out + (Y(in) - Y(out))` on every channel); either way the result is clamped
//! to `[0, 1]`. All sliders at zero change nothing, bit for bit.
//! **A choice, not compared with Photoshop**, whose algorithm is
//! unpublished: only the channel it moves is shifted (not its complement),
//! and the three weights do not sum to one (the Dodge/Burn convention).
//!
//! # Validation
//!
//! Every value finite and in `-100..=100`. A value outside is an error,
//! never clamped, including when decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: a tuple `(shadows, midtones, highlights,
//! preserve_luminosity)`, each range three `f32`s `(cyan-red,
//! magenta-green, yellow-blue)`, then one `bool`, 37 bytes.

/// The three tonal ranges' names, in storage order.
pub const COLOR_BALANCE_RANGES: [&str; 3] = ["Shadows", "Midtones", "Highlights"];

/// The largest slider magnitude.
pub const COLOR_BALANCE_MAX: f32 = 100.0;

/// Why a [`ColorBalanceParams`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorBalanceError {
    /// A value was `NaN` or infinite.
    NotFinite,
    /// A slider is outside `-100..=100`.
    Slider,
}

impl std::fmt::Display for ColorBalanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFinite => "a Color Balance value is not a finite number",
            Self::Slider => "a Color Balance slider is outside -100..100",
        })
    }
}

impl std::error::Error for ColorBalanceError {}

/// The wire form: see the module docs.
type Wire = ([f32; 3], [f32; 3], [f32; 3], bool);

/// A Color Balance adjustment's settings. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct ColorBalanceParams {
    /// `[range][cyan-red, magenta-green, yellow-blue]`, ranges in
    /// [`COLOR_BALANCE_RANGES`] order.
    ranges: [[f32; 3]; 3],
    preserve_luminosity: bool,
}

// Every value is finite (validated), so `==` is an equivalence.
impl Eq for ColorBalanceParams {}

impl Default for ColorBalanceParams {
    fn default() -> Self {
        Self::identity()
    }
}

impl TryFrom<Wire> for ColorBalanceParams {
    type Error = ColorBalanceError;

    fn try_from(v: Wire) -> Result<Self, ColorBalanceError> {
        Self::new([v.0, v.1, v.2], v.3)
    }
}

impl From<ColorBalanceParams> for Wire {
    fn from(p: ColorBalanceParams) -> Self {
        let [shadows, midtones, highlights] = p.ranges;
        (shadows, midtones, highlights, p.preserve_luminosity)
    }
}

impl ColorBalanceParams {
    /// Every slider at zero, Preserve Luminosity on (Photoshop's default):
    /// changes nothing.
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            ranges: [[0.0; 3]; 3],
            preserve_luminosity: true,
        }
    }

    /// A validated record.
    ///
    /// # Errors
    /// [`ColorBalanceError`] naming the first rule broken.
    pub fn new(
        ranges: [[f32; 3]; 3],
        preserve_luminosity: bool,
    ) -> Result<Self, ColorBalanceError> {
        let all = || ranges.iter().flatten();
        if all().any(|v| !v.is_finite()) {
            return Err(ColorBalanceError::NotFinite);
        }
        if all().any(|v| !(-COLOR_BALANCE_MAX..=COLOR_BALANCE_MAX).contains(v)) {
            return Err(ColorBalanceError::Slider);
        }
        Ok(Self {
            ranges,
            preserve_luminosity,
        })
    }

    /// The sliders of every range: `[range][cyan-red, magenta-green,
    /// yellow-blue]`.
    #[must_use]
    pub const fn ranges(&self) -> [[f32; 3]; 3] {
        self.ranges
    }

    /// The three sliders of range `range` (`0` shadows, `1` midtones, `2`
    /// highlights), `None` past the last.
    #[must_use]
    pub fn range(&self, range: usize) -> Option<[f32; 3]> {
        self.ranges.get(range).copied()
    }

    /// Whether the input's luma is restored after the shifts.
    #[must_use]
    pub const fn preserve_luminosity(&self) -> bool {
        self.preserve_luminosity
    }

    /// Whether the adjustment changes nothing: every slider exactly zero
    /// (Preserve Luminosity is irrelevant then).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.ranges.iter().flatten().all(|&v| v == 0.0)
    }

    /// A copy with slider `slider` (`0` cyan-red, `1` magenta-green, `2`
    /// yellow-blue) of range `range` replaced.
    ///
    /// # Errors
    /// As [`Self::new`]; an index past the last range or slider is
    /// [`ColorBalanceError::Slider`].
    pub fn with_slider(
        &self,
        range: usize,
        slider: usize,
        value: f32,
    ) -> Result<Self, ColorBalanceError> {
        let mut ranges = self.ranges;
        *ranges
            .get_mut(range)
            .and_then(|r| r.get_mut(slider))
            .ok_or(ColorBalanceError::Slider)? = value;
        Self::new(ranges, self.preserve_luminosity)
    }

    /// A copy with Preserve Luminosity switched.
    #[must_use]
    pub const fn with_preserve_luminosity(&self, preserve: bool) -> Self {
        Self {
            preserve_luminosity: preserve,
            ..*self
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{ColorBalanceError as E, ColorBalanceParams as P};

    fn ok(p: Result<P, E>) -> P {
        match p {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn identity_is_every_slider_at_zero_whatever_the_flag() {
        assert!(P::identity().is_identity());
        assert!(P::identity().preserve_luminosity());
        assert!(P::default().is_identity());
        assert!(P::identity().with_preserve_luminosity(false).is_identity());
        assert!(ok(P::new([[-0.0; 3]; 3], true)).is_identity());
        for r in 0..3 {
            for s in 0..3 {
                assert!(
                    !ok(P::identity().with_slider(r, s, 1.0)).is_identity(),
                    "{r} {s}"
                );
            }
        }
    }

    #[test]
    fn validation_names_the_rule_broken_at_the_boundaries() {
        assert!(P::new([[100.0, -100.0, 100.0]; 3], false).is_ok());
        for r in 0..3 {
            for s in 0..3 {
                assert_eq!(P::identity().with_slider(r, s, 100.5), Err(E::Slider));
                assert_eq!(P::identity().with_slider(r, s, -100.5), Err(E::Slider));
                for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                    assert_eq!(P::identity().with_slider(r, s, bad), Err(E::NotFinite));
                }
            }
        }
        assert_eq!(P::identity().with_slider(3, 0, 0.0), Err(E::Slider));
        assert_eq!(P::identity().with_slider(0, 3, 0.0), Err(E::Slider));
    }

    #[test]
    fn the_wire_form_round_trips_and_refuses_bad_values() {
        let p = ok(P::new(
            [[1.0, -2.0, 3.0], [-4.0, 5.0, -6.0], [7.0, -8.0, 100.0]],
            false,
        ));
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        assert_eq!(bytes.len(), 9 * 4 + 1, "nine f32s and a bool, positional");
        assert_eq!(bytes.get(36), Some(&0));
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
        for bad in [101.0_f32, -100.5, f32::NAN, f32::NEG_INFINITY] {
            // Each of the nine slots, one at a time.
            for slot in 0..9 {
                let mut v = [0.0_f32; 9];
                if let Some(x) = v.get_mut(slot) {
                    *x = bad;
                }
                let tuple = (
                    [v[0], v[1], v[2]],
                    [v[3], v[4], v[5]],
                    [v[6], v[7], v[8]],
                    true,
                );
                let Ok(wire) = postcard::to_allocvec(&tuple) else {
                    unreachable!("encodes");
                };
                assert!(postcard::from_bytes::<P>(&wire).is_err(), "{bad} in {slot}");
            }
        }
        assert!(postcard::from_bytes::<P>(bytes.get(..36).unwrap_or(&[])).is_err());
    }

    #[test]
    fn setters_edit_only_their_slot() {
        let p = ok(P::new(
            [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
            true,
        ));
        for r in 0..3 {
            for s in 0..3 {
                let q = ok(p.with_slider(r, s, -50.0));
                let mut want = p.ranges();
                want[r][s] = -50.0;
                assert_eq!(q.ranges(), want);
                assert!(q.preserve_luminosity());
            }
        }
        assert_eq!(p.range(1), Some([4.0, 5.0, 6.0]));
        assert_eq!(p.range(3), None);
        let q = p.with_preserve_luminosity(false);
        assert!(!q.preserve_luminosity());
        assert_eq!(q.ranges(), p.ranges());
    }
}
