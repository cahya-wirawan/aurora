//! The parameters of a Hue/Saturation adjustment (0.203.0): Photoshop's
//! master Hue, Saturation and Lightness sliders and its Colorize mode.
//! This type holds only the master and Colorize settings; the
//! per-range adjustments (reds, yellows, greens, cyans, blues, magentas)
//! live in [`crate::HueSaturationRangesParams`] (`hue_ranges.rs`).
//!
//! It lives in `aurora-core`, beside [`crate::LevelsParams`], so
//! `aurora-doc` can store it in a layer while `aurora-filters` applies it.
//!
//! # The math (applied by `aurora_filters::HueSaturationOp`)
//!
//! On **straight, encoded** RGB converted to HSL (hue in degrees,
//! saturation and lightness in `[0, 1]`; `l = (max + min) / 2`):
//!
//! ```text
//! master:    h' = h + hue                          (degrees, wraps)
//!            k  = saturation / 100
//!            s' = s * (1 + k)                      k <= 0  (towards grey)
//!            s' = min(1, s / (1 - k))              0 < k < 1
//!            s' = 1 if s > 0, else 0               k = 1
//!            t  = lightness / 100
//!            l' = l * (1 + t)                      t <= 0  (towards black)
//!            l' = l + (1 - l) * t                  t > 0   (towards white)
//! colorize:  h' = colorize_hue, s' = colorize_saturation / 100,
//!            l' = the same lightness rule with colorize_lightness
//! ```
//!
//! Evidence (see PLAN.md, 0.203.0): against Photoshop's own merged image
//! of psd-tools' `adjustments/huesaturation_colorize_rgb.psd` (two pure
//! Colorize regions) the Colorize model has a mean error of 0.4 levels
//! (max 3.5); the master model against `huesaturation_rgb.psd`'s
//! master-only layer (hue 122, saturation 100, lightness -37) has a mean
//! of about 1.0 level on pixels with HSL saturation above 0.4, and a
//! hue shift (124 degrees) against `clip-adjustment.psd` matches within
//! 0.03 levels. **Not verified**: the positive-saturation curve between 0
//! and 100 (the corpus holds only `+100`, where every non-grey pixel
//! saturates fully, and `+19` inside a stack Aurora cannot isolate), so
//! `s / (1 - k)` is a choice, not a measurement; and near-grey pixels,
//! where the corpus's backdrop reconstruction is too uncertain to judge.
//!
//! # Validation
//!
//! Every value finite; hue in `-180..=180`, saturation and lightness in
//! `-100..=100`; Colorize hue in `0..=360`, saturation `0..=100`,
//! lightness `-100..=100`. A value outside is an error, never clamped,
//! including when decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: a tuple of seven values `(hue, saturation,
//! lightness, colorize, colorize_hue, colorize_saturation,
//! colorize_lightness)`, `f32`s and one `bool`.

/// Why a [`HueSaturationParams`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HueSaturationError {
    /// A value was `NaN` or infinite.
    NotFinite,
    /// The master hue is outside `-180..=180`.
    Hue,
    /// The master saturation or lightness is outside `-100..=100`.
    MasterRange,
    /// The Colorize hue is outside `0..=360`.
    ColorizeHue,
    /// The Colorize saturation is outside `0..=100`.
    ColorizeSaturation,
    /// The Colorize lightness is outside `-100..=100`.
    ColorizeLightness,
}

impl std::fmt::Display for HueSaturationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFinite => "a Hue/Saturation value is not a finite number",
            Self::Hue => "the Hue/Saturation hue is outside -180..180",
            Self::MasterRange => "the Hue/Saturation saturation or lightness is outside -100..100",
            Self::ColorizeHue => "the Colorize hue is outside 0..360",
            Self::ColorizeSaturation => "the Colorize saturation is outside 0..100",
            Self::ColorizeLightness => "the Colorize lightness is outside -100..100",
        })
    }
}

impl std::error::Error for HueSaturationError {}

/// The wire form: see the module docs.
type Wire = (f32, f32, f32, bool, f32, f32, f32);

/// A Hue/Saturation adjustment's settings. See the module docs for the
/// math, the validation and the wire format.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct HueSaturationParams {
    hue: f32,
    saturation: f32,
    lightness: f32,
    colorize: bool,
    colorize_hue: f32,
    colorize_saturation: f32,
    colorize_lightness: f32,
}

// Every value is finite (validated), so `==` is an equivalence.
impl Eq for HueSaturationParams {}

impl Default for HueSaturationParams {
    fn default() -> Self {
        Self::identity()
    }
}

impl TryFrom<Wire> for HueSaturationParams {
    type Error = HueSaturationError;

    fn try_from(v: Wire) -> Result<Self, HueSaturationError> {
        Self::new(v.0, v.1, v.2, v.3, v.4, v.5, v.6)
    }
}

impl From<HueSaturationParams> for Wire {
    fn from(p: HueSaturationParams) -> Self {
        (
            p.hue,
            p.saturation,
            p.lightness,
            p.colorize,
            p.colorize_hue,
            p.colorize_saturation,
            p.colorize_lightness,
        )
    }
}

impl HueSaturationParams {
    /// Photoshop's defaults for the Colorize values when Colorize is off.
    pub const DEFAULT_COLORIZE_SATURATION: f32 = 25.0;

    /// Master sliders at zero, Colorize off (its values at Photoshop's
    /// defaults, hue 0, saturation 25, lightness 0): changes nothing.
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            hue: 0.0,
            saturation: 0.0,
            lightness: 0.0,
            colorize: false,
            colorize_hue: 0.0,
            colorize_saturation: Self::DEFAULT_COLORIZE_SATURATION,
            colorize_lightness: 0.0,
        }
    }

    /// A validated record. See the module docs for the rules.
    ///
    /// # Errors
    /// [`HueSaturationError`] naming the first rule broken.
    pub fn new(
        hue: f32,
        saturation: f32,
        lightness: f32,
        colorize: bool,
        colorize_hue: f32,
        colorize_saturation: f32,
        colorize_lightness: f32,
    ) -> Result<Self, HueSaturationError> {
        let all = [
            hue,
            saturation,
            lightness,
            colorize_hue,
            colorize_saturation,
            colorize_lightness,
        ];
        if all.iter().any(|v| !v.is_finite()) {
            return Err(HueSaturationError::NotFinite);
        }
        if !(-180.0..=180.0).contains(&hue) {
            return Err(HueSaturationError::Hue);
        }
        if !(-100.0..=100.0).contains(&saturation) || !(-100.0..=100.0).contains(&lightness) {
            return Err(HueSaturationError::MasterRange);
        }
        if !(0.0..=360.0).contains(&colorize_hue) {
            return Err(HueSaturationError::ColorizeHue);
        }
        if !(0.0..=100.0).contains(&colorize_saturation) {
            return Err(HueSaturationError::ColorizeSaturation);
        }
        if !(-100.0..=100.0).contains(&colorize_lightness) {
            return Err(HueSaturationError::ColorizeLightness);
        }
        Ok(Self {
            hue,
            saturation,
            lightness,
            colorize,
            colorize_hue,
            colorize_saturation,
            colorize_lightness,
        })
    }

    /// The master hue shift in degrees, `-180..=180`.
    #[must_use]
    pub const fn hue(&self) -> f32 {
        self.hue
    }

    /// The master saturation, `-100..=100`.
    #[must_use]
    pub const fn saturation(&self) -> f32 {
        self.saturation
    }

    /// The master lightness, `-100..=100`.
    #[must_use]
    pub const fn lightness(&self) -> f32 {
        self.lightness
    }

    /// Whether Colorize is on (the master sliders are then not applied).
    #[must_use]
    pub const fn colorize(&self) -> bool {
        self.colorize
    }

    /// The Colorize hue in degrees, `0..=360`.
    #[must_use]
    pub const fn colorize_hue(&self) -> f32 {
        self.colorize_hue
    }

    /// The Colorize saturation, `0..=100`.
    #[must_use]
    pub const fn colorize_saturation(&self) -> f32 {
        self.colorize_saturation
    }

    /// The Colorize lightness, `-100..=100`.
    #[must_use]
    pub const fn colorize_lightness(&self) -> f32 {
        self.colorize_lightness
    }

    /// Whether the adjustment changes nothing: Colorize off and the three
    /// master sliders exactly zero (the Colorize values are then unused).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        !self.colorize && self.hue == 0.0 && self.saturation == 0.0 && self.lightness == 0.0
    }

    /// A copy with the master hue replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_hue(&self, hue: f32) -> Result<Self, HueSaturationError> {
        Self::new(
            hue,
            self.saturation,
            self.lightness,
            self.colorize,
            self.colorize_hue,
            self.colorize_saturation,
            self.colorize_lightness,
        )
    }

    /// A copy with the master saturation replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_saturation(&self, saturation: f32) -> Result<Self, HueSaturationError> {
        Self::new(
            self.hue,
            saturation,
            self.lightness,
            self.colorize,
            self.colorize_hue,
            self.colorize_saturation,
            self.colorize_lightness,
        )
    }

    /// A copy with the master lightness replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_lightness(&self, lightness: f32) -> Result<Self, HueSaturationError> {
        Self::new(
            self.hue,
            self.saturation,
            lightness,
            self.colorize,
            self.colorize_hue,
            self.colorize_saturation,
            self.colorize_lightness,
        )
    }

    /// A copy with Colorize switched on or off (its values kept).
    #[must_use]
    pub const fn with_colorize(&self, colorize: bool) -> Self {
        Self { colorize, ..*self }
    }

    /// A copy with the Colorize hue replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_colorize_hue(&self, hue: f32) -> Result<Self, HueSaturationError> {
        Self::new(
            self.hue,
            self.saturation,
            self.lightness,
            self.colorize,
            hue,
            self.colorize_saturation,
            self.colorize_lightness,
        )
    }

    /// A copy with the Colorize saturation replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_colorize_saturation(&self, saturation: f32) -> Result<Self, HueSaturationError> {
        Self::new(
            self.hue,
            self.saturation,
            self.lightness,
            self.colorize,
            self.colorize_hue,
            saturation,
            self.colorize_lightness,
        )
    }

    /// A copy with the Colorize lightness replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_colorize_lightness(&self, lightness: f32) -> Result<Self, HueSaturationError> {
        Self::new(
            self.hue,
            self.saturation,
            self.lightness,
            self.colorize,
            self.colorize_hue,
            self.colorize_saturation,
            lightness,
        )
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{HueSaturationError as E, HueSaturationParams as P};

    fn ok(p: Result<P, E>) -> P {
        match p {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn identity_is_detected_and_colorize_values_do_not_matter_when_off() {
        assert!(P::identity().is_identity());
        assert!(P::default().is_identity());
        let off = ok(P::new(0.0, 0.0, 0.0, false, 200.0, 80.0, -20.0));
        assert!(off.is_identity());
        assert!(!off.with_colorize(true).is_identity());
        assert!(!ok(P::identity().with_hue(1.0)).is_identity());
        assert!(!ok(P::identity().with_saturation(-0.5)).is_identity());
        assert!(!ok(P::identity().with_lightness(3.0)).is_identity());
    }

    #[test]
    fn validation_names_the_first_rule_broken_at_the_boundaries() {
        assert!(P::new(-180.0, -100.0, -100.0, true, 0.0, 0.0, -100.0).is_ok());
        assert!(P::new(180.0, 100.0, 100.0, true, 360.0, 100.0, 100.0).is_ok());
        let base = (0.0, 0.0, 0.0, false, 0.0, 25.0, 0.0);
        let with = |i: usize, v: f32| {
            let mut a = [base.0, base.1, base.2, base.4, base.5, base.6];
            if let Some(slot) = a.get_mut(i) {
                *slot = v;
            }
            let [h, sa, li, ch, cs, cl] = a;
            P::new(h, sa, li, false, ch, cs, cl)
        };
        assert_eq!(with(0, 180.5), Err(E::Hue));
        assert_eq!(with(0, -181.0), Err(E::Hue));
        assert_eq!(with(1, 100.5), Err(E::MasterRange));
        assert_eq!(with(2, -100.5), Err(E::MasterRange));
        assert_eq!(with(3, -0.5), Err(E::ColorizeHue));
        assert_eq!(with(3, 360.5), Err(E::ColorizeHue));
        assert_eq!(with(4, -1.0), Err(E::ColorizeSaturation));
        assert_eq!(with(4, 100.5), Err(E::ColorizeSaturation));
        assert_eq!(with(5, 101.0), Err(E::ColorizeLightness));
        // 0.204.0 judge nits: the half-step boundaries and exactly +-100.
        assert_eq!(with(5, 100.5), Err(E::ColorizeLightness));
        assert_eq!(with(5, -100.5), Err(E::ColorizeLightness));
        assert!(P::new(0.0, 100.0, 100.0, false, 0.0, 25.0, 0.0).is_ok());
        assert!(P::new(0.0, -100.0, -100.0, false, 0.0, 25.0, 0.0).is_ok());
        assert!(P::new(0.0, 0.0, 0.0, true, 0.0, 25.0, 100.0).is_ok());
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for i in 0..6 {
                assert_eq!(with(i, bad), Err(E::NotFinite), "{i} {bad}");
            }
        }
    }

    #[test]
    fn the_wire_form_round_trips_and_refuses_bad_values() {
        let p = ok(P::new(-27.0, 12.0, -64.0, true, 242.0, 45.0, 14.0));
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        // Six f32s and a bool, positional, no framing.
        assert_eq!(bytes.len(), 6 * 4 + 1);
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
        let Ok(bad) = postcard::to_allocvec(&(
            200.0_f32, 0.0_f32, 0.0_f32, false, 0.0_f32, 25.0_f32, 0.0_f32,
        )) else {
            unreachable!("encodes");
        };
        assert!(postcard::from_bytes::<P>(&bad).is_err(), "decode validates");
        assert!(postcard::from_bytes::<P>(bytes.get(..bytes.len() - 1).unwrap_or(&[])).is_err());
    }

    #[test]
    fn setters_keep_the_other_values_and_validate() {
        let p = ok(P::new(10.0, 20.0, 30.0, true, 40.0, 50.0, 60.0));
        assert_eq!(ok(p.with_hue(11.0)).hue(), 11.0);
        assert_eq!(ok(p.with_saturation(21.0)).saturation(), 21.0);
        assert_eq!(ok(p.with_lightness(31.0)).lightness(), 31.0);
        assert_eq!(ok(p.with_colorize_hue(41.0)).colorize_hue(), 41.0);
        assert_eq!(
            ok(p.with_colorize_saturation(51.0)).colorize_saturation(),
            51.0
        );
        assert_eq!(
            ok(p.with_colorize_lightness(61.0)).colorize_lightness(),
            61.0
        );
        assert!(!p.with_colorize(false).colorize());
        let q = ok(p.with_hue(11.0));
        assert_eq!(
            (q.saturation(), q.lightness(), q.colorize()),
            (20.0, 30.0, true)
        );
        assert!(p.with_hue(999.0).is_err());
    }
}
