//! The parameters and exact math of a Levels adjustment (0.202.0): one
//! master (composite RGB) [`LevelsChannel`] plus one per colour channel,
//! which is Photoshop's own Levels model (its `levl` block stores a
//! composite record and a record per channel).
//!
//! It lives in `aurora-core`, beside [`crate::CurvesParams`], for the same
//! layering reason: `aurora-doc` stores it in a layer and
//! `aurora-filters` builds the lookup tables that apply it.
//!
//! # The math
//!
//! One channel maps a value `x` (straight, encoded, `[0, 1]`) by
//!
//! ```text
//! t   = clamp((x - input_black) / (input_white - input_black), 0, 1)
//! g   = t ^ (1 / gamma)                       // gamma 1.0 is neutral
//! out = clamp(output_black + g * (output_white - output_black), 0, 1)
//! ```
//!
//! which is psd-tools' Photoshop-checked `apply_levels`. **Order of
//! application** is per-channel first, then the master:
//! `out_c = master(channel_c(in_c))`, the same order as Curves
//! ([`crate::curves`]) and psd-tools' `_apply_luts` ("individual
//! adjustments get applied independently of each other, then the master
//! lut is applied"). Evidence: `adjustments/levels_rgb.psd` (four Levels
//! layers, each with a master and channel records) composites to within
//! a mean of well under one level of Photoshop's own merged image with
//! this order (see PLAN.md, 0.202.0).
//!
//! # Validation
//!
//! A [`LevelsChannel`] is only constructible through [`LevelsChannel::new`]
//! (and the validating `Deserialize`): every value finite; the input
//! black in `[0, 1)` and white in `(0, 1]` with white at least
//! [`MIN_INPUT_SPAN`] (one 8-bit level) above black; the output points in
//! `[0, 1]` (they **may cross**, which inverts the tone, as the
//! formula allows; not checked against Photoshop's slider behaviour);
//! gamma in [`GAMMA_MIN`]`..=`[`GAMMA_MAX`] (Photoshop's 0.10..9.99). A
//! value outside is an error, never silently clamped, including when
//! decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: [`LevelsParams`] is four channels in
//! declaration order (master, red, green, blue), each a tuple of five
//! `f32` `(input_black, input_white, gamma, output_black, output_white)`.

/// The smallest `input_white - input_black`: one 8-bit level, the least
/// Photoshop's sliders allow.
pub const MIN_INPUT_SPAN: f32 = 1.0 / 255.0;

/// Photoshop's lowest midtone gamma.
pub const GAMMA_MIN: f32 = 0.10;

/// Photoshop's highest midtone gamma.
pub const GAMMA_MAX: f32 = 9.99;

/// Why a [`LevelsChannel`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelsError {
    /// A value was `NaN` or infinite.
    NotFinite,
    /// An input or output point is outside `[0, 1]`.
    OutOfRange,
    /// The input white is not at least [`MIN_INPUT_SPAN`] above the black.
    InputSpan,
    /// The gamma is outside [`GAMMA_MIN`]`..=`[`GAMMA_MAX`].
    Gamma,
}

/// One Levels record: input black/white, midtone gamma, output
/// black/white. See the module docs for the math and the validation.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(
    try_from = "(f32, f32, f32, f32, f32)",
    into = "(f32, f32, f32, f32, f32)"
)]
pub struct LevelsChannel {
    input_black: f32,
    input_white: f32,
    gamma: f32,
    output_black: f32,
    output_white: f32,
}

// All five values are finite (validated), so `==` is an equivalence.
impl Eq for LevelsChannel {}

impl Default for LevelsChannel {
    fn default() -> Self {
        Self::identity()
    }
}

impl TryFrom<(f32, f32, f32, f32, f32)> for LevelsChannel {
    type Error = LevelsError;

    fn try_from(v: (f32, f32, f32, f32, f32)) -> Result<Self, LevelsError> {
        Self::new(v.0, v.1, v.2, v.3, v.4)
    }
}

impl From<LevelsChannel> for (f32, f32, f32, f32, f32) {
    fn from(c: LevelsChannel) -> Self {
        (
            c.input_black,
            c.input_white,
            c.gamma,
            c.output_black,
            c.output_white,
        )
    }
}

impl std::fmt::Display for LevelsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFinite => "a Levels value is not a finite number",
            Self::OutOfRange => "a Levels input or output point is outside 0..1",
            Self::InputSpan => "the Levels input white must be above the input black",
            Self::Gamma => "the Levels gamma is outside 0.10..9.99",
        })
    }
}

impl LevelsChannel {
    /// Black 0, white 1, gamma 1, output 0..1: changes nothing.
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            input_black: 0.0,
            input_white: 1.0,
            gamma: 1.0,
            output_black: 0.0,
            output_white: 1.0,
        }
    }

    /// A validated record. See the module docs for the rules.
    ///
    /// # Errors
    /// [`LevelsError`] naming the first rule broken.
    pub fn new(
        input_black: f32,
        input_white: f32,
        gamma: f32,
        output_black: f32,
        output_white: f32,
    ) -> Result<Self, LevelsError> {
        let all = [input_black, input_white, gamma, output_black, output_white];
        if all.iter().any(|v| !v.is_finite()) {
            return Err(LevelsError::NotFinite);
        }
        let unit = |v: f32| (0.0..=1.0).contains(&v);
        if !(unit(input_black) && unit(input_white) && unit(output_black) && unit(output_white)) {
            return Err(LevelsError::OutOfRange);
        }
        // Compare with a hair of slack so 1/255-spaced values built from
        // 8-bit levels (`253 / 255`, `254 / 255`) are not refused by
        // rounding.
        if input_white - input_black < MIN_INPUT_SPAN * 0.999 {
            return Err(LevelsError::InputSpan);
        }
        if !(GAMMA_MIN..=GAMMA_MAX).contains(&gamma) {
            return Err(LevelsError::Gamma);
        }
        Ok(Self {
            input_black,
            input_white,
            gamma,
            output_black,
            output_white,
        })
    }

    /// The input black point, `[0, 1)`.
    #[must_use]
    pub const fn input_black(&self) -> f32 {
        self.input_black
    }

    /// The input white point, `(0, 1]`.
    #[must_use]
    pub const fn input_white(&self) -> f32 {
        self.input_white
    }

    /// The midtone gamma, `0.10..=9.99`; `1.0` is neutral, above 1
    /// brightens the midtones.
    #[must_use]
    pub const fn gamma(&self) -> f32 {
        self.gamma
    }

    /// The output black point, `[0, 1]`.
    #[must_use]
    pub const fn output_black(&self) -> f32 {
        self.output_black
    }

    /// The output white point, `[0, 1]`.
    #[must_use]
    pub const fn output_white(&self) -> f32 {
        self.output_white
    }

    /// Whether this record changes nothing (exactly the defaults).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        *self == Self::identity()
    }

    /// The input stage: `t = clamp((x - black) / (white - black), 0, 1)`;
    /// `NaN` reads as `0`.
    #[must_use]
    pub fn normalised_input(&self, x: f32) -> f32 {
        if x.is_nan() {
            return 0.0;
        }
        ((x - self.input_black) / (self.input_white - self.input_black)).clamp(0.0, 1.0)
    }

    /// The output stage: `clamp(black + g * (white - black), 0, 1)`.
    #[must_use]
    pub fn output_of(&self, g: f32) -> f32 {
        g.mul_add(self.output_white - self.output_black, self.output_black)
            .clamp(0.0, 1.0)
    }

    /// The exact (not tabulated) map of one value; the reference every
    /// lookup table is measured against. An identity record passes `x`
    /// through unchanged, even out of range.
    #[must_use]
    pub fn map(&self, x: f32) -> f32 {
        if self.is_identity() {
            return x;
        }
        let t = self.normalised_input(x);
        self.output_of(t.powf(1.0 / self.gamma))
    }
}

/// A Levels adjustment's parameters: a master record and one per colour
/// channel. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct LevelsParams {
    /// Applied to every colour channel after that channel's own record.
    pub master: LevelsChannel,
    /// The red channel's own record.
    pub red: LevelsChannel,
    /// The green channel's own record.
    pub green: LevelsChannel,
    /// The blue channel's own record.
    pub blue: LevelsChannel,
}

impl LevelsParams {
    /// Every record the identity: the adjustment changes nothing.
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            master: LevelsChannel::identity(),
            red: LevelsChannel::identity(),
            green: LevelsChannel::identity(),
            blue: LevelsChannel::identity(),
        }
    }

    /// The record for colour channel `index` (`0` red, `1` green, `2`
    /// blue), or `None` for any other index.
    #[must_use]
    pub const fn channel(&self, index: usize) -> Option<&LevelsChannel> {
        match index {
            0 => Some(&self.red),
            1 => Some(&self.green),
            2 => Some(&self.blue),
            _ => None,
        }
    }

    /// Whether every record is the identity (an exact passthrough).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.master.is_identity()
            && self.red.is_identity()
            && self.green.is_identity()
            && self.blue.is_identity()
    }

    /// The exact map of one value of colour channel `index`: its own
    /// record, then the master.
    #[must_use]
    pub fn map_channel(&self, index: usize, x: f32) -> f32 {
        let after_channel = self.channel(index).map_or(x, |c| c.map(x));
        self.master.map(after_channel)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{GAMMA_MAX, GAMMA_MIN, LevelsChannel, LevelsError, LevelsParams, MIN_INPUT_SPAN};

    fn channel(ib: f32, iw: f32, g: f32, ob: f32, ow: f32) -> LevelsChannel {
        match LevelsChannel::new(ib, iw, g, ob, ow) {
            Ok(c) => c,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn identity_is_exact_and_detected() {
        let id = LevelsChannel::identity();
        assert!(id.is_identity());
        assert!(LevelsParams::identity().is_identity());
        for x in [0.0, 0.123_456_7, 1.0, 1.5, -0.25] {
            assert_eq!(id.map(x).to_bits(), x.to_bits(), "{x}");
        }
        let mut p = LevelsParams::identity();
        p.green = channel(0.0, 1.0, 1.0, 0.0, 0.9);
        assert!(!p.is_identity());
        assert!(p.channel(1).is_some_and(|c| !c.is_identity()));
        assert!(p.channel(3).is_none());
    }

    #[test]
    fn extremes_and_input_clip() {
        let c = channel(0.2, 0.8, 1.0, 0.1, 0.9);
        assert!(
            (c.map(0.0) - 0.1).abs() < 1e-6,
            "below black -> output black"
        );
        assert!((c.map(0.2) - 0.1).abs() < 1e-6);
        assert!((c.map(0.5) - 0.5).abs() < 1e-6, "midpoint of a linear map");
        assert!((c.map(0.8) - 0.9).abs() < 1e-6);
        assert!(
            (c.map(1.0) - 0.9).abs() < 1e-6,
            "above white -> output white"
        );
        assert!((c.map(f32::NAN) - 0.1).abs() < 1e-6, "NaN reads as 0");
    }

    #[test]
    fn gamma_brightens_above_one_and_darkens_below() {
        let bright = channel(0.0, 1.0, 2.0, 0.0, 1.0);
        let dark = channel(0.0, 1.0, 0.5, 0.0, 1.0);
        assert!((bright.map(0.25) - 0.5).abs() < 1e-6, "0.25 ^ (1/2)");
        assert!((dark.map(0.5) - 0.25).abs() < 1e-6, "0.5 ^ 2");
        // gamma never moves the endpoints
        for g in [GAMMA_MIN, 0.5, 3.0, GAMMA_MAX] {
            let c = channel(0.0, 1.0, g, 0.0, 1.0);
            assert_eq!(c.map(0.0), 0.0);
            assert_eq!(c.map(1.0), 1.0);
        }
    }

    #[test]
    fn crossed_output_inverts() {
        let c = channel(0.0, 1.0, 1.0, 1.0, 0.0);
        assert!((c.map(0.25) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn channel_then_master_order() {
        let mut p = LevelsParams::identity();
        p.red = channel(0.0, 0.5, 1.0, 0.0, 1.0); // doubles, clips at .5
        p.master = channel(0.0, 1.0, 1.0, 0.0, 0.5); // halves
        // 0.25 -> red 0.5 -> master 0.25; the other order would give 0.125 -> 0.25 too,
        // so use a clipping input: 0.75 -> red 1.0 -> master 0.5 (reverse: 0.375 -> 0.75).
        assert!((p.map_channel(0, 0.75) - 0.5).abs() < 1e-6);
        assert!(
            (p.map_channel(1, 0.75) - 0.375).abs() < 1e-6,
            "green only master"
        );
    }

    #[test]
    fn validation_refuses_every_bad_value() {
        let nan = f32::NAN;
        assert_eq!(
            LevelsChannel::new(nan, 1.0, 1.0, 0.0, 1.0),
            Err(LevelsError::NotFinite)
        );
        assert_eq!(
            LevelsChannel::new(0.0, f32::INFINITY, 1.0, 0.0, 1.0),
            Err(LevelsError::NotFinite)
        );
        assert_eq!(
            LevelsChannel::new(-0.1, 1.0, 1.0, 0.0, 1.0),
            Err(LevelsError::OutOfRange)
        );
        assert_eq!(
            LevelsChannel::new(0.0, 1.1, 1.0, 0.0, 1.0),
            Err(LevelsError::OutOfRange)
        );
        assert_eq!(
            LevelsChannel::new(0.0, 1.0, 1.0, 0.0, 1.5),
            Err(LevelsError::OutOfRange)
        );
        assert_eq!(
            LevelsChannel::new(0.5, 0.5, 1.0, 0.0, 1.0),
            Err(LevelsError::InputSpan)
        );
        assert_eq!(
            LevelsChannel::new(0.6, 0.5, 1.0, 0.0, 1.0),
            Err(LevelsError::InputSpan)
        );
        assert_eq!(
            LevelsChannel::new(0.0, MIN_INPUT_SPAN * 0.5, 1.0, 0.0, 1.0),
            Err(LevelsError::InputSpan)
        );
        assert!(LevelsChannel::new(253.0 / 255.0, 1.0, 1.0, 0.0, 1.0).is_ok());
        assert_eq!(
            LevelsChannel::new(0.0, 1.0, 0.09, 0.0, 1.0),
            Err(LevelsError::Gamma)
        );
        assert_eq!(
            LevelsChannel::new(0.0, 1.0, 10.0, 0.0, 1.0),
            Err(LevelsError::Gamma)
        );
        assert!(LevelsChannel::new(0.0, 1.0, GAMMA_MIN, 0.0, 1.0).is_ok());
        assert!(LevelsChannel::new(0.0, 1.0, GAMMA_MAX, 0.0, 1.0).is_ok());
    }

    #[test]
    fn postcard_round_trips_and_refuses_an_invalid_record() {
        let params = LevelsParams {
            master: channel(0.1, 0.9, 1.4, 0.05, 0.95),
            red: channel(0.0, 1.0, 0.7, 0.0, 1.0),
            ..LevelsParams::identity()
        };
        let bytes = match postcard::to_allocvec(&params) {
            Ok(b) => b,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(bytes.len(), 4 * 20, "four records of five f32");
        match postcard::from_bytes::<LevelsParams>(&bytes) {
            Ok(back) => assert_eq!(back, params),
            Err(err) => unreachable!("{err:?}"),
        }
        // Each bad tuple is a decode error, never a repaired record.
        for bad in [
            (0.5_f32, 0.5, 1.0, 0.0, 1.0),
            (0.0, 1.0, 20.0, 0.0, 1.0),
            (f32::NAN, 1.0, 1.0, 0.0, 1.0),
            (0.0, 1.0, 1.0, 0.0, 2.0),
        ] {
            let bytes = match postcard::to_allocvec(&bad) {
                Ok(b) => b,
                Err(err) => unreachable!("{err:?}"),
            };
            assert!(
                postcard::from_bytes::<LevelsChannel>(&bytes).is_err(),
                "{bad:?}"
            );
        }
        assert!(postcard::from_bytes::<LevelsParams>(bytes.get(..79).unwrap_or(&[])).is_err());
    }
}
