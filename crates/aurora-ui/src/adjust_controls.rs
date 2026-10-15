//! The Properties panel's **Vibrance, Exposure, Photo Filter, Black & White
//! and Color Balance editors** (0.239.0, the last two 0.240.0): one hidden
//! column holding five groups of sliders with a readout each (and the
//! checkboxes and the Color Balance range selector), of which at most one
//! is shown, the one matching the active layer.
//!
//! 0.244.0 adds four more groups: Invert (a note, no sliders), Threshold
//! (one level slider), Posterize (one levels slider) and Brightness/Contrast
//! (two sliders and a Use Legacy checkbox; the sliders span both modes and
//! an edit is clamped to the layer's mode).
//!
//! **Where it lives.** Inside the Properties panel's tool-controls strip
//! beside the Hue/Saturation editor ([`crate::insert_tool_controls`]), so
//! every pointer, key and accessibility event on it reaches `aurora-app`
//! through the same `ToolControls` routing the other editors use. It
//! starts hidden (`Display::None`) and disabled.
//!
//! **This module knows nothing about undo or pixels**: it builds the
//! widgets, maps a slider to a field of an [`Adjustment`]
//! ([`AdjustSlider::with_value`]) and mirrors a document into the widgets
//! ([`sync_adjust_controls`]). One undo step per drag and per checkbox
//! toggle is `aurora-app`'s job (`adjust_app`).
//!
//! **Units.** Vibrance and Saturation in whole percent; Exposure in stops
//! to two decimals, Offset to three, Gamma to two (Photoshop's fields);
//! Density in whole percent; the Photo Filter colour as three 8-bit
//! channel sliders (`0..=255`), because the properties panel has no
//! colour picker. The presets (`Warming (85)` and the others) are palette
//! commands, not widgets here. Black & White: six whole-percent colour
//! sliders, a Tint checkbox and the tint hue (degrees) and saturation
//! (percent) sliders, which are live only while Tint is on. Color Balance:
//! a three-tab range selector (Shadows, Midtones, Highlights; widget state
//! only, never document state, no undo step, like the Hue/Saturation range
//! selector), three whole-number sliders that edit the selected range, and
//! a Preserve Luminosity checkbox.

// `AdjustControls` is `Copy` and 400 bytes (twenty sliders and readouts since
// 0.240.0); the helpers take it by value like the other editors' controls.
#![allow(clippy::large_types_passed_by_value)]

use aurora_core::{
    BLACK_WHITE_DEFAULTS, BLACK_WHITE_MAX, BLACK_WHITE_MIN, BRIGHTNESS_MAX, BlackWhiteParams,
    BrightnessContrastParams, CHANNEL_MIXER_CONSTANT_MAX, CHANNEL_MIXER_OUTPUTS,
    CHANNEL_MIXER_SOURCE_MAX, COLOR_BALANCE_MAX, COLOR_BALANCE_RANGES, CONTRAST_MAX,
    DEFAULT_TINT_HUE, DEFAULT_TINT_SATURATION, EXPOSURE_MAX, ExposureParams, GAMMA_MAX, GAMMA_MIN,
    LEGACY_MAX, OFFSET_MAX, POSTERIZE_DEFAULT, POSTERIZE_MAX, POSTERIZE_MIN, PhotoFilterParams,
    SELECTIVE_COLOR_MAX, SELECTIVE_COLOR_RANGES, THRESHOLD_DEFAULT, THRESHOLD_MAX, THRESHOLD_MIN,
    VibranceParams,
};
use aurora_doc::{Adjustment, LayerId, LayerTree};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{Display, FlexDirection, Size, Style};

/// How many sliders the editors hold.
pub const ADJUST_SLIDER_COUNT: usize = 38;
/// How many editor groups there are.
pub const ADJUST_KIND_COUNT: usize = 12;

/// Which of the adjustments an editor group belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdjustKind {
    Vibrance,
    Exposure,
    PhotoFilter,
    BlackWhite,
    ColorBalance,
    /// Invert (0.244.0): no settings, a note instead of sliders.
    Invert,
    /// Threshold (0.244.0): one level slider.
    Threshold,
    /// Posterize (0.244.0): one levels slider.
    Posterize,
    /// Brightness/Contrast (0.244.0): two sliders and Use Legacy.
    BrightnessContrast,
    /// Channel Mixer (0.245.0): an output selector, three source sliders, a
    /// constant and Monochrome.
    ChannelMixer,
    /// Selective Color (0.245.0): a nine-range selector, four ink sliders
    /// and the Absolute method.
    SelectiveColor,
    /// Gradient Map (0.248.0): a summary row, Reverse, and Start and End
    /// colour sliders that are live only for a two-stop gradient.
    GradientMap,
}

impl AdjustKind {
    /// Every kind, in group order.
    pub const ALL: [Self; ADJUST_KIND_COUNT] = [
        Self::Vibrance,
        Self::Exposure,
        Self::PhotoFilter,
        Self::BlackWhite,
        Self::ColorBalance,
        Self::Invert,
        Self::Threshold,
        Self::Posterize,
        Self::BrightnessContrast,
        Self::ChannelMixer,
        Self::SelectiveColor,
        Self::GradientMap,
    ];

    /// Position in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Vibrance => 0,
            Self::Exposure => 1,
            Self::PhotoFilter => 2,
            Self::BlackWhite => 3,
            Self::ColorBalance => 4,
            Self::Invert => 5,
            Self::Threshold => 6,
            Self::Posterize => 7,
            Self::BrightnessContrast => 8,
            Self::ChannelMixer => 9,
            Self::SelectiveColor => 10,
            Self::GradientMap => 11,
        }
    }

    /// The kind of adjustment `adjustment` is, if it is one of these.
    #[must_use]
    pub const fn of(adjustment: &Adjustment) -> Option<Self> {
        match adjustment {
            Adjustment::Vibrance(_) => Some(Self::Vibrance),
            Adjustment::Exposure(_) => Some(Self::Exposure),
            Adjustment::PhotoFilter(_) => Some(Self::PhotoFilter),
            Adjustment::BlackWhite(_) => Some(Self::BlackWhite),
            Adjustment::ColorBalance(_) => Some(Self::ColorBalance),
            Adjustment::Invert => Some(Self::Invert),
            Adjustment::Threshold(_) => Some(Self::Threshold),
            Adjustment::Posterize(_) => Some(Self::Posterize),
            Adjustment::BrightnessContrast(_) => Some(Self::BrightnessContrast),
            Adjustment::ChannelMixer(_) => Some(Self::ChannelMixer),
            Adjustment::SelectiveColor(_) => Some(Self::SelectiveColor),
            Adjustment::GradientMap(_) => Some(Self::GradientMap),
            _ => None,
        }
    }
}

/// One of the thirty-eight sliders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdjustSlider {
    /// Vibrance, `-100..=100`.
    Vibrance,
    /// The Vibrance layer's saturation, `-100..=100`.
    VibranceSaturation,
    /// Exposure in stops, `-20..=20`.
    Exposure,
    /// Offset, `-0.5..=0.5`.
    Offset,
    /// Gamma correction, `0.10..=9.99`.
    Gamma,
    /// Photo Filter density, `1..=100`.
    Density,
    /// The filter colour's red level, `0..=255`.
    Red,
    /// The filter colour's green level, `0..=255`.
    Green,
    /// The filter colour's blue level, `0..=255`.
    Blue,
    /// Black & White's six colour sliders, `-200..=300`.
    BwReds,
    BwYellows,
    BwGreens,
    BwCyans,
    BwBlues,
    BwMagentas,
    /// The Black & White tint hue, degrees `0..=360`.
    TintHue,
    /// The Black & White tint saturation, `0..=100`.
    TintSaturation,
    /// Color Balance's cyan-red slider of the selected range, `-100..=100`.
    BalanceCyanRed,
    /// Its magenta-green slider.
    BalanceMagentaGreen,
    /// Its yellow-blue slider.
    BalanceYellowBlue,
    /// Threshold's level, `1..=255`.
    ThresholdLevel,
    /// Posterize's level count, `2..=255`.
    PosterizeLevels,
    /// Brightness/Contrast's brightness. [`Self::range`] is the widest
    /// (`-150..=150`); [`sync_adjust_controls`] narrows the built slider to
    /// the layer's mode (`-100..=100` with Use Legacy).
    Brightness,
    /// Its contrast. [`Self::range`] is the widest (`-100..=100`); the built
    /// slider follows the mode (`-50..=100` in the default mode).
    Contrast,
    /// Channel Mixer's red source of the selected output (the Gray mix
    /// while Monochrome is on), `-200..=200`.
    MixerRed,
    /// Its green source.
    MixerGreen,
    /// Its blue source.
    MixerBlue,
    /// Its constant, `-100..=100`.
    MixerConstant,
    /// Selective Color's cyan slider of the selected range, `-100..=100`.
    SelectiveCyan,
    /// Its magenta slider.
    SelectiveMagenta,
    /// Its yellow slider.
    SelectiveYellow,
    /// Its black slider.
    SelectiveBlack,
    /// A two-stop Gradient Map's start colour, `0..=255` per channel.
    GradientStartRed,
    GradientStartGreen,
    GradientStartBlue,
    /// Its end colour.
    GradientEndRed,
    GradientEndGreen,
    GradientEndBlue,
}

impl AdjustSlider {
    /// Every slider, in layout order — the one place that order is fixed.
    pub const ALL: [Self; ADJUST_SLIDER_COUNT] = [
        Self::Vibrance,
        Self::VibranceSaturation,
        Self::Exposure,
        Self::Offset,
        Self::Gamma,
        Self::Density,
        Self::Red,
        Self::Green,
        Self::Blue,
        Self::BwReds,
        Self::BwYellows,
        Self::BwGreens,
        Self::BwCyans,
        Self::BwBlues,
        Self::BwMagentas,
        Self::TintHue,
        Self::TintSaturation,
        Self::BalanceCyanRed,
        Self::BalanceMagentaGreen,
        Self::BalanceYellowBlue,
        Self::ThresholdLevel,
        Self::PosterizeLevels,
        Self::Brightness,
        Self::Contrast,
        Self::MixerRed,
        Self::MixerGreen,
        Self::MixerBlue,
        Self::MixerConstant,
        Self::SelectiveCyan,
        Self::SelectiveMagenta,
        Self::SelectiveYellow,
        Self::SelectiveBlack,
        Self::GradientStartRed,
        Self::GradientStartGreen,
        Self::GradientStartBlue,
        Self::GradientEndRed,
        Self::GradientEndGreen,
        Self::GradientEndBlue,
    ];

    /// Position in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Vibrance => 0,
            Self::VibranceSaturation => 1,
            Self::Exposure => 2,
            Self::Offset => 3,
            Self::Gamma => 4,
            Self::Density => 5,
            Self::Red => 6,
            Self::Green => 7,
            Self::Blue => 8,
            Self::BwReds => 9,
            Self::BwYellows => 10,
            Self::BwGreens => 11,
            Self::BwCyans => 12,
            Self::BwBlues => 13,
            Self::BwMagentas => 14,
            Self::TintHue => 15,
            Self::TintSaturation => 16,
            Self::BalanceCyanRed => 17,
            Self::BalanceMagentaGreen => 18,
            Self::BalanceYellowBlue => 19,
            Self::ThresholdLevel => 20,
            Self::PosterizeLevels => 21,
            Self::Brightness => 22,
            Self::Contrast => 23,
            Self::MixerRed => 24,
            Self::MixerGreen => 25,
            Self::MixerBlue => 26,
            Self::MixerConstant => 27,
            Self::SelectiveCyan => 28,
            Self::SelectiveMagenta => 29,
            Self::SelectiveYellow => 30,
            Self::SelectiveBlack => 31,
            Self::GradientStartRed => 32,
            Self::GradientStartGreen => 33,
            Self::GradientStartBlue => 34,
            Self::GradientEndRed => 35,
            Self::GradientEndGreen => 36,
            Self::GradientEndBlue => 37,
        }
    }

    /// The group the slider belongs to.
    #[must_use]
    pub const fn kind(self) -> AdjustKind {
        match self {
            Self::Vibrance | Self::VibranceSaturation => AdjustKind::Vibrance,
            Self::Exposure | Self::Offset | Self::Gamma => AdjustKind::Exposure,
            Self::Density | Self::Red | Self::Green | Self::Blue => AdjustKind::PhotoFilter,
            Self::BwReds
            | Self::BwYellows
            | Self::BwGreens
            | Self::BwCyans
            | Self::BwBlues
            | Self::BwMagentas
            | Self::TintHue
            | Self::TintSaturation => AdjustKind::BlackWhite,
            Self::BalanceCyanRed | Self::BalanceMagentaGreen | Self::BalanceYellowBlue => {
                AdjustKind::ColorBalance
            }
            Self::ThresholdLevel => AdjustKind::Threshold,
            Self::PosterizeLevels => AdjustKind::Posterize,
            Self::Brightness | Self::Contrast => AdjustKind::BrightnessContrast,
            Self::MixerRed | Self::MixerGreen | Self::MixerBlue | Self::MixerConstant => {
                AdjustKind::ChannelMixer
            }
            Self::SelectiveCyan
            | Self::SelectiveMagenta
            | Self::SelectiveYellow
            | Self::SelectiveBlack => AdjustKind::SelectiveColor,
            Self::GradientStartRed
            | Self::GradientStartGreen
            | Self::GradientStartBlue
            | Self::GradientEndRed
            | Self::GradientEndGreen
            | Self::GradientEndBlue => AdjustKind::GradientMap,
        }
    }

    /// Whether the slider is one of the two tint sliders, live only while
    /// the Black & White Tint checkbox is on.
    #[must_use]
    pub const fn is_tint(self) -> bool {
        matches!(self, Self::TintHue | Self::TintSaturation)
    }

    /// The Black & White colour slider's slot (`0..6`, reds first), `None`
    /// for every other slider.
    #[must_use]
    pub const fn bw_slot(self) -> Option<usize> {
        match self {
            Self::BwReds => Some(0),
            Self::BwYellows => Some(1),
            Self::BwGreens => Some(2),
            Self::BwCyans => Some(3),
            Self::BwBlues => Some(4),
            Self::BwMagentas => Some(5),
            _ => None,
        }
    }

    /// The Color Balance slider's slot (`0` cyan-red, `1` magenta-green,
    /// `2` yellow-blue), `None` for every other slider.
    #[must_use]
    pub const fn balance_slot(self) -> Option<usize> {
        match self {
            Self::BalanceCyanRed => Some(0),
            Self::BalanceMagentaGreen => Some(1),
            Self::BalanceYellowBlue => Some(2),
            _ => None,
        }
    }

    /// The Channel Mixer slider's slot in a row (`0` red, `1` green, `2`
    /// blue, `3` constant), `None` for every other slider.
    #[must_use]
    pub const fn mixer_slot(self) -> Option<usize> {
        match self {
            Self::MixerRed => Some(0),
            Self::MixerGreen => Some(1),
            Self::MixerBlue => Some(2),
            Self::MixerConstant => Some(3),
            _ => None,
        }
    }

    /// The Selective Color slider's slot (`0` cyan, `1` magenta, `2`
    /// yellow, `3` black), `None` for every other slider.
    #[must_use]
    pub const fn selective_slot(self) -> Option<usize> {
        match self {
            Self::SelectiveCyan => Some(0),
            Self::SelectiveMagenta => Some(1),
            Self::SelectiveYellow => Some(2),
            Self::SelectiveBlack => Some(3),
            _ => None,
        }
    }

    /// The Gradient Map slider's `(end, channel)`: `end` is `false` for the
    /// start colour and `true` for the end colour, `channel` `0` red, `1`
    /// green, `2` blue; `None` for every other slider.
    #[must_use]
    pub const fn gradient_slot(self) -> Option<(bool, usize)> {
        match self {
            Self::GradientStartRed => Some((false, 0)),
            Self::GradientStartGreen => Some((false, 1)),
            Self::GradientStartBlue => Some((false, 2)),
            Self::GradientEndRed => Some((true, 0)),
            Self::GradientEndGreen => Some((true, 1)),
            Self::GradientEndBlue => Some((true, 2)),
            _ => None,
        }
    }

    /// The slider's accessible name (unique among the editors' sliders).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Vibrance => "Vibrance",
            Self::VibranceSaturation => "Vibrance saturation",
            Self::Exposure => "Exposure",
            Self::Offset => "Exposure offset",
            Self::Gamma => "Exposure gamma",
            Self::Density => "Filter density",
            Self::Red => "Filter red",
            Self::Green => "Filter green",
            Self::Blue => "Filter blue",
            Self::BwReds => "Black and white reds",
            Self::BwYellows => "Black and white yellows",
            Self::BwGreens => "Black and white greens",
            Self::BwCyans => "Black and white cyans",
            Self::BwBlues => "Black and white blues",
            Self::BwMagentas => "Black and white magentas",
            Self::TintHue => "Black and white tint hue",
            Self::TintSaturation => "Black and white tint saturation",
            Self::BalanceCyanRed => "Color balance cyan to red",
            Self::BalanceMagentaGreen => "Color balance magenta to green",
            Self::BalanceYellowBlue => "Color balance yellow to blue",
            Self::ThresholdLevel => "Threshold level",
            Self::PosterizeLevels => "Posterize levels",
            Self::Brightness => "Brightness",
            Self::Contrast => "Contrast",
            Self::MixerRed => "Channel mixer red source",
            Self::MixerGreen => "Channel mixer green source",
            Self::MixerBlue => "Channel mixer blue source",
            Self::MixerConstant => "Channel mixer constant",
            Self::SelectiveCyan => "Selective color cyan",
            Self::SelectiveMagenta => "Selective color magenta",
            Self::SelectiveYellow => "Selective color yellow",
            Self::SelectiveBlack => "Selective color black",
            Self::GradientStartRed => "Gradient map start red",
            Self::GradientStartGreen => "Gradient map start green",
            Self::GradientStartBlue => "Gradient map start blue",
            Self::GradientEndRed => "Gradient map end red",
            Self::GradientEndGreen => "Gradient map end green",
            Self::GradientEndBlue => "Gradient map end blue",
        }
    }

    /// The readout's prefix.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Vibrance => "Vibrance",
            Self::VibranceSaturation => "Saturation",
            Self::Exposure => "Exposure",
            Self::Offset => "Offset",
            Self::Gamma => "Gamma",
            Self::Density => "Density",
            Self::Red | Self::MixerRed => "Red",
            Self::Green | Self::MixerGreen => "Green",
            Self::Blue | Self::MixerBlue => "Blue",
            Self::BwReds => "Reds",
            Self::BwYellows => "Yellows",
            Self::BwGreens => "Greens",
            Self::BwCyans => "Cyans",
            Self::BwBlues => "Blues",
            Self::BwMagentas => "Magentas",
            Self::TintHue => "Tint hue",
            Self::TintSaturation => "Tint saturation",
            Self::BalanceCyanRed => "Cyan-Red",
            Self::BalanceMagentaGreen => "Magenta-Green",
            Self::BalanceYellowBlue => "Yellow-Blue",
            Self::ThresholdLevel => "Threshold Level",
            Self::PosterizeLevels => "Levels",
            Self::Brightness => "Brightness",
            Self::Contrast => "Contrast",
            Self::MixerConstant => "Constant",
            Self::SelectiveCyan => "Cyan",
            Self::SelectiveMagenta => "Magenta",
            Self::SelectiveYellow => "Yellow",
            Self::SelectiveBlack => "Black",
            Self::GradientStartRed => "Start red",
            Self::GradientStartGreen => "Start green",
            Self::GradientStartBlue => "Start blue",
            Self::GradientEndRed => "End red",
            Self::GradientEndGreen => "End green",
            Self::GradientEndBlue => "End blue",
        }
    }

    /// The slider's `(min, max)`.
    #[must_use]
    pub const fn range(self) -> (f64, f64) {
        match self {
            Self::Vibrance | Self::VibranceSaturation => (-100.0, 100.0),
            Self::Exposure => (-(EXPOSURE_MAX as f64), EXPOSURE_MAX as f64),
            Self::Offset => (-(OFFSET_MAX as f64), OFFSET_MAX as f64),
            Self::Gamma => (GAMMA_MIN as f64, GAMMA_MAX as f64),
            Self::Density => (1.0, 100.0),
            Self::Red
            | Self::Green
            | Self::Blue
            | Self::GradientStartRed
            | Self::GradientStartGreen
            | Self::GradientStartBlue
            | Self::GradientEndRed
            | Self::GradientEndGreen
            | Self::GradientEndBlue => (0.0, 255.0),
            Self::BwReds
            | Self::BwYellows
            | Self::BwGreens
            | Self::BwCyans
            | Self::BwBlues
            | Self::BwMagentas => (BLACK_WHITE_MIN as f64, BLACK_WHITE_MAX as f64),
            Self::TintHue => (0.0, 360.0),
            Self::TintSaturation => (0.0, 100.0),
            Self::BalanceCyanRed | Self::BalanceMagentaGreen | Self::BalanceYellowBlue => {
                (-(COLOR_BALANCE_MAX as f64), COLOR_BALANCE_MAX as f64)
            }
            Self::ThresholdLevel => (THRESHOLD_MIN as f64, THRESHOLD_MAX as f64),
            Self::PosterizeLevels => (POSTERIZE_MIN as f64, POSTERIZE_MAX as f64),
            Self::Brightness => (-(BRIGHTNESS_MAX as f64), BRIGHTNESS_MAX as f64),
            Self::Contrast => (-(LEGACY_MAX as f64), CONTRAST_MAX as f64),
            Self::MixerRed | Self::MixerGreen | Self::MixerBlue => (
                -(CHANNEL_MIXER_SOURCE_MAX as f64),
                CHANNEL_MIXER_SOURCE_MAX as f64,
            ),
            Self::MixerConstant => (
                -(CHANNEL_MIXER_CONSTANT_MAX as f64),
                CHANNEL_MIXER_CONSTANT_MAX as f64,
            ),
            Self::SelectiveCyan
            | Self::SelectiveMagenta
            | Self::SelectiveYellow
            | Self::SelectiveBlack => (-(SELECTIVE_COLOR_MAX as f64), SELECTIVE_COLOR_MAX as f64),
        }
    }

    /// Decimal places the slider's value is kept to (and its readout shows).
    #[must_use]
    pub const fn decimals(self) -> usize {
        match self {
            Self::Exposure | Self::Gamma => 2,
            Self::Offset => 3,
            _ => 0,
        }
    }

    /// The position a freshly built slider starts at (before a document is
    /// mirrored into it).
    #[must_use]
    pub fn initial(self) -> f64 {
        match self {
            Self::Gamma => 1.0,
            Self::Density => f64::from(aurora_core::DEFAULT_DENSITY),
            Self::TintHue => f64::from(DEFAULT_TINT_HUE),
            Self::TintSaturation => f64::from(DEFAULT_TINT_SATURATION),
            Self::ThresholdLevel => f64::from(THRESHOLD_DEFAULT),
            Self::PosterizeLevels => f64::from(POSTERIZE_DEFAULT),
            Self::GradientEndRed | Self::GradientEndGreen | Self::GradientEndBlue => 255.0,
            other => other
                .bw_slot()
                .and_then(|slot| BLACK_WHITE_DEFAULTS.get(slot))
                .map_or(0.0, |&w| f64::from(w)),
        }
    }

    /// The readout text for `value`: `+1.50` style for Exposure and the
    /// Color Balance sliders, plain otherwise.
    #[must_use]
    pub fn readout(self, value: f64) -> String {
        let places = self.decimals();
        if matches!(self, Self::Exposure | Self::Brightness | Self::Contrast)
            || self.balance_slot().is_some()
            || self.mixer_slot().is_some()
            || self.selective_slot().is_some()
        {
            format!("{}: {value:+.places$}", self.short_label())
        } else {
            format!("{}: {value:.places$}", self.short_label())
        }
    }

    /// The slider position for `adjustment`, `None` when it is not this
    /// slider's adjustment. `range` is the tab of the selector of the
    /// slider's own kind ([`adjust_tab`]): Color Balance's range (`0`
    /// shadows, `1` midtones, `2` highlights), Channel Mixer's output (`0`
    /// red, `1` green, `2` blue; ignored while Monochrome is on, when the
    /// first row is the Gray mix) or Selective Color's range (`0` Reds to `8`
    /// Blacks); every other slider ignores it.
    #[must_use]
    pub fn value_of(self, adjustment: &Adjustment, range: usize) -> Option<f64> {
        let channel = |i: usize| {
            adjustment
                .photo_filter()
                .and_then(|p| p.colour().get(i).copied())
                .map(|c| f64::from((c * 255.0).round()))
        };
        match self {
            Self::Vibrance => adjustment.vibrance().map(|p| f64::from(p.vibrance())),
            Self::VibranceSaturation => adjustment.vibrance().map(|p| f64::from(p.saturation())),
            Self::Exposure => adjustment.exposure().map(|p| f64::from(p.exposure())),
            Self::Offset => adjustment.exposure().map(|p| f64::from(p.offset())),
            Self::Gamma => adjustment.exposure().map(|p| f64::from(p.gamma())),
            Self::Density => adjustment.photo_filter().map(|p| f64::from(p.density())),
            Self::Red => channel(0),
            Self::Green => channel(1),
            Self::Blue => channel(2),
            Self::TintHue => adjustment.black_white().map(|p| f64::from(p.tint_hue())),
            Self::TintSaturation => adjustment
                .black_white()
                .map(|p| f64::from(p.tint_saturation())),
            Self::BwReds
            | Self::BwYellows
            | Self::BwGreens
            | Self::BwCyans
            | Self::BwBlues
            | Self::BwMagentas => {
                let weights = adjustment.black_white()?.weights();
                weights.get(self.bw_slot()?).copied().map(f64::from)
            }
            Self::BalanceCyanRed | Self::BalanceMagentaGreen | Self::BalanceYellowBlue => {
                let sliders = adjustment.color_balance()?.range(range)?;
                sliders.get(self.balance_slot()?).copied().map(f64::from)
            }
            Self::ThresholdLevel => adjustment.threshold().map(|p| f64::from(p.level())),
            Self::PosterizeLevels => adjustment.posterize().map(|p| f64::from(p.levels())),
            Self::Brightness => adjustment
                .brightness_contrast()
                .map(|p| f64::from(p.brightness())),
            Self::Contrast => adjustment
                .brightness_contrast()
                .map(|p| f64::from(p.contrast())),
            Self::MixerRed | Self::MixerGreen | Self::MixerBlue | Self::MixerConstant => {
                let p = adjustment.channel_mixer()?;
                let output = if p.monochrome() { 0 } else { range };
                p.row(output)?
                    .get(self.mixer_slot()?)
                    .copied()
                    .map(f64::from)
            }
            Self::SelectiveCyan
            | Self::SelectiveMagenta
            | Self::SelectiveYellow
            | Self::SelectiveBlack => {
                let sliders = adjustment.selective_color()?.range(range)?;
                sliders.get(self.selective_slot()?).copied().map(f64::from)
            }
            Self::GradientStartRed
            | Self::GradientStartGreen
            | Self::GradientStartBlue
            | Self::GradientEndRed
            | Self::GradientEndGreen
            | Self::GradientEndBlue => {
                // Only a two-stop gradient has a start and an end colour to
                // edit; any other shows its summary and nothing here.
                let p = adjustment.gradient_map().filter(|p| p.is_two_stop())?;
                let (end, channel) = self.gradient_slot()?;
                let stop = p.stops().get(usize::from(end))?;
                stop.colour
                    .get(channel)
                    .map(|c| f64::from((c * 255.0).round()))
            }
        }
    }

    /// `value` rounded to the slider's decimals and clamped to its range.
    #[must_use]
    pub fn snap(self, value: f64) -> f64 {
        let (min, max) = self.range();
        let scale = 10_f64.powi(i32::try_from(self.decimals()).unwrap_or(0));
        ((value * scale).round() / scale).clamp(min, max)
    }

    /// `adjustment` with this slider moved to `value` (snapped), `None`
    /// for a non-finite `value` or an adjustment of another kind. `range`
    /// is as for [`Self::value_of`].
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
    pub fn with_value(
        self,
        adjustment: &Adjustment,
        range: usize,
        value: f64,
    ) -> Option<Adjustment> {
        if !value.is_finite() {
            return None;
        }
        let v = self.snap(value) as f32;
        match self {
            Self::Vibrance | Self::VibranceSaturation => {
                let p: VibranceParams = adjustment.vibrance()?;
                let next = if self == Self::Vibrance {
                    p.with_vibrance(v)
                } else {
                    p.with_saturation(v)
                };
                next.ok().map(Adjustment::Vibrance)
            }
            Self::Exposure | Self::Offset | Self::Gamma => {
                let p: ExposureParams = adjustment.exposure()?;
                let next = match self {
                    Self::Exposure => p.with_exposure(v),
                    Self::Offset => p.with_offset(v),
                    _ => p.with_gamma(v),
                };
                next.ok().map(Adjustment::Exposure)
            }
            Self::Density => adjustment
                .photo_filter()?
                .with_density(v)
                .ok()
                .map(Adjustment::PhotoFilter),
            Self::Red | Self::Green | Self::Blue => {
                let p: PhotoFilterParams = adjustment.photo_filter()?;
                let mut colour = p.colour();
                let slot = colour.get_mut(match self {
                    Self::Red => 0,
                    Self::Green => 1,
                    _ => 2,
                })?;
                *slot = v / 255.0;
                p.with_colour(colour).ok().map(Adjustment::PhotoFilter)
            }
            Self::TintHue | Self::TintSaturation => {
                let p: BlackWhiteParams = adjustment.black_white()?;
                let next = if self == Self::TintHue {
                    p.with_tint_hue(v)
                } else {
                    p.with_tint_saturation(v)
                };
                next.ok().map(Adjustment::BlackWhite)
            }
            Self::BwReds
            | Self::BwYellows
            | Self::BwGreens
            | Self::BwCyans
            | Self::BwBlues
            | Self::BwMagentas => adjustment
                .black_white()?
                .with_weight(self.bw_slot()?, v)
                .ok()
                .map(Adjustment::BlackWhite),
            Self::BalanceCyanRed | Self::BalanceMagentaGreen | Self::BalanceYellowBlue => {
                adjustment
                    .color_balance()?
                    .with_slider(range, self.balance_slot()?, v)
                    .ok()
                    .map(Adjustment::ColorBalance)
            }
            Self::ThresholdLevel => {
                // `snap` clamped `v` into 1..=255.
                #[allow(clippy::cast_sign_loss)]
                let level = v as u8;
                adjustment.threshold()?;
                aurora_core::ThresholdParams::new(level)
                    .ok()
                    .map(Adjustment::Threshold)
            }
            Self::PosterizeLevels => {
                #[allow(clippy::cast_sign_loss)]
                let levels = v as u8;
                adjustment.posterize()?;
                aurora_core::PosterizeParams::new(levels)
                    .ok()
                    .map(Adjustment::Posterize)
            }
            Self::Brightness | Self::Contrast => {
                let p = adjustment.brightness_contrast()?;
                // The slider spans both modes; an edit stays in the mode's range.
                let ((b0, b1), (c0, c1)) = BrightnessContrastParams::ranges(p.legacy());
                let n = v as i16;
                if self == Self::Brightness {
                    p.with_brightness(n.clamp(b0, b1))
                } else {
                    p.with_contrast(n.clamp(c0, c1))
                }
                .ok()
                .map(Adjustment::BrightnessContrast)
            }
            Self::MixerRed | Self::MixerGreen | Self::MixerBlue | Self::MixerConstant => {
                let p = adjustment.channel_mixer()?;
                let output = if p.monochrome() { 0 } else { range };
                p.with_value(output, self.mixer_slot()?, v)
                    .ok()
                    .map(Adjustment::ChannelMixer)
            }
            Self::SelectiveCyan
            | Self::SelectiveMagenta
            | Self::SelectiveYellow
            | Self::SelectiveBlack => adjustment
                .selective_color()?
                .with_slider(range, self.selective_slot()?, v)
                .ok()
                .map(Adjustment::SelectiveColor),
            Self::GradientStartRed
            | Self::GradientStartGreen
            | Self::GradientStartBlue
            | Self::GradientEndRed
            | Self::GradientEndGreen
            | Self::GradientEndBlue => {
                let p = adjustment.gradient_map().filter(|p| p.is_two_stop())?;
                let (end, channel) = self.gradient_slot()?;
                p.with_stop_channel(usize::from(end), channel, v / 255.0)
                    .ok()
                    .map(Adjustment::GradientMap)
            }
        }
    }
}

/// `id`'s adjustment when it is one of the editors' kinds.
#[must_use]
pub fn adjust_of(layers: &LayerTree, id: LayerId) -> Option<&Adjustment> {
    layers
        .adjustment(id)
        .filter(|adjustment| AdjustKind::of(adjustment).is_some())
}

/// The tab labels of the Color Balance range selector.
const BALANCE_TAB_LABELS: [&str; 3] = ["Shad", "Mid", "High"];

/// The tab labels of the Channel Mixer output selector.
const MIXER_TAB_LABELS: [&str; 3] = ["Red", "Green", "Blue"];

/// The tab labels of Selective Color's two selectors. Nine tabs do not fit
/// the panel, so the range is chosen in two steps: a group bar (the six hue
/// ranges together, then Whites, Neutrals, Blacks), and below it, live only
/// for the hue group, a bar of the six hues.
const SELECTIVE_GROUP_LABELS: [&str; 4] = ["Colors", "Whites", "Neutrals", "Blacks"];
const SELECTIVE_HUE_LABELS: [&str; 6] = ["Red", "Yel", "Grn", "Cyn", "Blu", "Mag"];

/// The sliders, their readouts, the checkboxes, the Color Balance range
/// selector and the containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdjustControls {
    /// The column container.
    pub root: WidgetId,
    /// The per-kind groups, indexed by [`AdjustKind::index`].
    pub groups: [WidgetId; ADJUST_KIND_COUNT],
    /// The sliders, indexed by [`AdjustSlider::index`].
    pub sliders: [WidgetId; ADJUST_SLIDER_COUNT],
    /// Their readout labels, same order.
    pub readouts: [WidgetId; ADJUST_SLIDER_COUNT],
    /// Photo Filter's "Preserve Luminosity" checkbox.
    pub preserve: WidgetId,
    /// Black & White's "Tint" checkbox.
    pub tint: WidgetId,
    /// Color Balance's "Preserve Luminosity" checkbox.
    pub balance_preserve: WidgetId,
    /// Color Balance's range selector tab bar: tab 0 Shadows, 1 Midtones,
    /// 2 Highlights.
    pub balance_range: WidgetId,
    /// Brightness/Contrast's "Use Legacy" checkbox (0.244.0).
    pub legacy: WidgetId,
    /// The line Invert's group shows in place of sliders (0.244.0).
    pub invert_note: WidgetId,
    /// Channel Mixer's output selector tab bar (0.245.0): tab 0 Red, 1
    /// Green, 2 Blue; idle while Monochrome is on.
    pub mixer_output: WidgetId,
    /// Channel Mixer's "Monochrome" checkbox.
    pub mixer_mono: WidgetId,
    /// Selective Color's group selector tab bar: tab 0 the six colour
    /// ranges, 1 Whites, 2 Neutrals, 3 Blacks.
    pub selective_group: WidgetId,
    /// Selective Color's colour selector tab bar (live for group 0): tab 0
    /// Reds to 5 Magentas.
    pub selective_range: WidgetId,
    /// Selective Color's "Absolute" checkbox (off: Relative).
    pub selective_absolute: WidgetId,
    /// Gradient Map's "N colour stops" summary row (0.248.0).
    pub gradient_summary: WidgetId,
    /// Gradient Map's "Reverse" checkbox.
    pub gradient_reverse: WidgetId,
}

impl AdjustControls {
    /// Which slider `id` is, if any.
    #[must_use]
    pub fn slider_of(&self, id: WidgetId) -> Option<AdjustSlider> {
        AdjustSlider::ALL
            .into_iter()
            .find(|slider| self.sliders.get(slider.index()) == Some(&id))
    }
}

fn column_style(scales: &Scales, display: Display) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let gap = length(scales.spacing.xs as f32);
    Style {
        display,
        flex_direction: FlexDirection::Column,
        flex_shrink: 0.0,
        gap: Size {
            width: gap,
            height: gap,
        },
        ..Default::default()
    }
}

/// Adds the hidden, disabled editors as the last child of `parent`.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `parent` doesn't exist. On any
/// failure the partly built column is removed again.
#[allow(clippy::too_many_lines)] // one widget per slider, in layout order
pub fn insert_adjust_controls(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
) -> Result<AdjustControls, WidgetError> {
    let root = widgets::insert_container(tree, parent, column_style(scales, Display::None))?;
    let built = (|| {
        let mut groups = Vec::new();
        for _ in AdjustKind::ALL {
            groups.push(widgets::insert_container(
                tree,
                root,
                column_style(scales, Display::None),
            )?);
        }
        let group_of = |kind: AdjustKind| {
            groups
                .get(kind.index())
                .copied()
                .ok_or(WidgetError::UnknownWidget(root))
        };
        let mut sliders = Vec::new();
        let mut readouts = Vec::new();
        let mut preserve = None;
        let mut tint = None;
        let mut balance_preserve = None;
        let mut balance_range = None;
        let mut legacy = None;
        let mut mixer_output = None;
        let mut mixer_mono = None;
        let mut selective_group = None;
        let mut selective_range = None;
        let mut selective_absolute = None;
        let mut gradient_summary = None;
        let mut gradient_reverse = None;
        let invert_note = widgets::insert_label(
            tree,
            group_of(AdjustKind::Invert)?,
            scales,
            "Invert has no settings.".to_owned(),
        )?;
        for slider in AdjustSlider::ALL {
            let group = group_of(slider.kind())?;
            // Controls that sit before a slider in its group.
            if slider == AdjustSlider::GradientStartRed {
                gradient_summary = Some(widgets::insert_label(
                    tree,
                    group,
                    scales,
                    "2 colour stops".to_owned(),
                )?);
            }
            if slider == AdjustSlider::TintHue {
                tint = Some(widgets::insert_checkbox(tree, group, scales, "Tint")?);
            }
            if slider == AdjustSlider::BalanceCyanRed {
                let bar = widgets::insert_tab_bar(
                    tree,
                    group,
                    scales,
                    "Color Balance range",
                    BALANCE_TAB_LABELS.iter().map(|&l| l.to_owned()).collect(),
                    0,
                )?;
                widgets::set_tab_accessible_names(
                    tree,
                    bar,
                    COLOR_BALANCE_RANGES.iter().map(|&n| n.to_owned()).collect(),
                )?;
                balance_range = Some(bar);
            }
            if slider == AdjustSlider::MixerRed {
                let bar = widgets::insert_tab_bar(
                    tree,
                    group,
                    scales,
                    "Channel Mixer output",
                    MIXER_TAB_LABELS.iter().map(|&l| l.to_owned()).collect(),
                    0,
                )?;
                widgets::set_tab_accessible_names(
                    tree,
                    bar,
                    CHANNEL_MIXER_OUTPUTS
                        .iter()
                        .map(|&n| format!("{n} output"))
                        .collect(),
                )?;
                mixer_output = Some(bar);
            }
            if slider == AdjustSlider::SelectiveCyan {
                let group_bar = widgets::insert_tab_bar(
                    tree,
                    group,
                    scales,
                    "Selective Color group",
                    SELECTIVE_GROUP_LABELS
                        .iter()
                        .map(|&l| l.to_owned())
                        .collect(),
                    0,
                )?;
                widgets::set_tab_accessible_names(
                    tree,
                    group_bar,
                    vec![
                        "Colors".to_owned(),
                        "Whites".to_owned(),
                        "Neutrals".to_owned(),
                        "Blacks".to_owned(),
                    ],
                )?;
                selective_group = Some(group_bar);
                let bar = widgets::insert_tab_bar(
                    tree,
                    group,
                    scales,
                    "Selective Color range",
                    SELECTIVE_HUE_LABELS.iter().map(|&l| l.to_owned()).collect(),
                    0,
                )?;
                widgets::set_tab_accessible_names(
                    tree,
                    bar,
                    SELECTIVE_COLOR_RANGES
                        .iter()
                        .take(6)
                        .map(|&n| n.to_owned())
                        .collect(),
                )?;
                selective_range = Some(bar);
            }
            let (min, max) = slider.range();
            let start = slider.initial();
            readouts.push(widgets::insert_label(
                tree,
                group,
                scales,
                slider.readout(start),
            )?);
            sliders.push(widgets::insert_slider(
                tree,
                group,
                scales,
                slider.label(),
                start,
                min,
                max,
            )?);
            if slider == AdjustSlider::Blue {
                preserve = Some(widgets::insert_checkbox(
                    tree,
                    group,
                    scales,
                    "Preserve Luminosity",
                )?);
            }
            if slider == AdjustSlider::Contrast {
                legacy = Some(widgets::insert_checkbox(tree, group, scales, "Use Legacy")?);
            }
            if slider == AdjustSlider::MixerConstant {
                mixer_mono = Some(widgets::insert_checkbox(tree, group, scales, "Monochrome")?);
            }
            if slider == AdjustSlider::GradientEndBlue {
                gradient_reverse = Some(widgets::insert_checkbox(tree, group, scales, "Reverse")?);
            }
            if slider == AdjustSlider::SelectiveBlack {
                selective_absolute =
                    Some(widgets::insert_checkbox(tree, group, scales, "Absolute")?);
            }
            if slider == AdjustSlider::BalanceYellowBlue {
                balance_preserve = Some(widgets::insert_checkbox(
                    tree,
                    group,
                    scales,
                    "Preserve Luminosity",
                )?);
            }
        }
        let (
            Some(preserve),
            Some(tint),
            Some(balance_preserve),
            Some(balance_range),
            Some(legacy),
            Some(mixer_output),
            Some(mixer_mono),
            Some(selective_group),
            Some(selective_range),
            Some(selective_absolute),
            Some(gradient_summary),
            Some(gradient_reverse),
            Ok(sliders),
            Ok(readouts),
            Ok(groups),
        ) = (
            preserve,
            tint,
            balance_preserve,
            balance_range,
            legacy,
            mixer_output,
            mixer_mono,
            selective_group,
            selective_range,
            selective_absolute,
            gradient_summary,
            gradient_reverse,
            <[WidgetId; ADJUST_SLIDER_COUNT]>::try_from(sliders),
            <[WidgetId; ADJUST_SLIDER_COUNT]>::try_from(readouts),
            <[WidgetId; ADJUST_KIND_COUNT]>::try_from(groups),
        )
        else {
            return Err(WidgetError::UnknownWidget(root));
        };
        let controls = AdjustControls {
            root,
            groups,
            sliders,
            readouts,
            preserve,
            tint,
            balance_preserve,
            balance_range,
            legacy,
            invert_note,
            mixer_output,
            mixer_mono,
            selective_group,
            selective_range,
            selective_absolute,
            gradient_summary,
            gradient_reverse,
        };
        set_shown(tree, controls, None, false, false, false)?;
        Ok(controls)
    })();
    if built.is_err() {
        let _ = tree.remove(root);
    }
    built
}

fn set_display(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    display: Display,
) -> Result<bool, WidgetError> {
    let mut style = tree
        .style(id)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(id))?;
    if style.display == display {
        return Ok(false);
    }
    style.display = display;
    tree.set_style(id, style)?;
    Ok(true)
}

/// Enables the checkbox `id` iff `live`; returns whether that changed it.
fn set_checkbox_live(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    live: bool,
) -> Result<bool, WidgetError> {
    let disabled = match tree.payload(id) {
        Some(WidgetKind::Checkbox(state)) => state.disabled,
        Some(_) => return Err(WidgetError::WrongWidgetKind(id)),
        None => return Err(WidgetError::UnknownWidget(id)),
    };
    if disabled == live {
        widgets::set_checkbox_disabled(tree, id, !live)?;
        return Ok(true);
    }
    Ok(false)
}

/// Shows exactly the group of `kind` (none: the whole column hides) and
/// enables only its widgets; the two Black & White tint sliders are live
/// only when `tint_on`.
#[allow(clippy::too_many_lines)] // one block per widget kind
fn set_shown(
    tree: &mut WidgetTree<WidgetKind>,
    controls: AdjustControls,
    kind: Option<AdjustKind>,
    tint_on: bool,
    mixer_mono: bool,
    gradient_colours: bool,
) -> Result<bool, WidgetError> {
    let selective_group = widgets::tab_bar_state(tree, controls.selective_group)
        .map_or(0, aurora_widgets::widgets::TabBarState::selected);
    let mut changed = set_display(
        tree,
        controls.root,
        if kind.is_some() {
            Display::Flex
        } else {
            Display::None
        },
    )?;
    for each in AdjustKind::ALL {
        let live = kind == Some(each);
        if let Some(&group) = controls.groups.get(each.index()) {
            changed |= set_display(
                tree,
                group,
                if live { Display::Flex } else { Display::None },
            )?;
        }
    }
    for slider in AdjustSlider::ALL {
        let (Some(&id), Some(&readout)) = (
            controls.sliders.get(slider.index()),
            controls.readouts.get(slider.index()),
        ) else {
            continue;
        };
        let live = kind == Some(slider.kind())
            && (!slider.is_tint() || tint_on)
            && (slider.gradient_slot().is_none() || gradient_colours);
        let disabled = match tree.payload(id) {
            Some(WidgetKind::Slider(state)) => state.disabled,
            Some(_) => return Err(WidgetError::WrongWidgetKind(id)),
            None => return Err(WidgetError::UnknownWidget(id)),
        };
        if disabled == live {
            widgets::set_slider_disabled(tree, id, !live)?;
            changed = true;
        }
        changed |= widgets::set_label_disabled(tree, readout, !live)?;
    }
    changed |= set_checkbox_live(
        tree,
        controls.preserve,
        kind == Some(AdjustKind::PhotoFilter),
    )?;
    changed |= set_checkbox_live(tree, controls.tint, kind == Some(AdjustKind::BlackWhite))?;
    changed |= set_checkbox_live(
        tree,
        controls.balance_preserve,
        kind == Some(AdjustKind::ColorBalance),
    )?;
    changed |= set_checkbox_live(
        tree,
        controls.legacy,
        kind == Some(AdjustKind::BrightnessContrast),
    )?;
    changed |=
        widgets::set_label_disabled(tree, controls.invert_note, kind != Some(AdjustKind::Invert))?;
    changed |= set_checkbox_live(
        tree,
        controls.mixer_mono,
        kind == Some(AdjustKind::ChannelMixer),
    )?;
    changed |= set_checkbox_live(
        tree,
        controls.selective_absolute,
        kind == Some(AdjustKind::SelectiveColor),
    )?;
    changed |= set_checkbox_live(
        tree,
        controls.gradient_reverse,
        kind == Some(AdjustKind::GradientMap),
    )?;
    changed |= widgets::set_label_disabled(
        tree,
        controls.gradient_summary,
        kind != Some(AdjustKind::GradientMap),
    )?;
    // The selectors are widget state only; each is live for its own kind
    // (the Mixer's output selector is idle while Monochrome is on: the Gray
    // mix is the first row).
    for (bar, tabs_live) in [
        (
            controls.balance_range,
            kind == Some(AdjustKind::ColorBalance),
        ),
        (
            controls.mixer_output,
            kind == Some(AdjustKind::ChannelMixer) && !mixer_mono,
        ),
        (
            controls.selective_group,
            kind == Some(AdjustKind::SelectiveColor),
        ),
        (
            controls.selective_range,
            kind == Some(AdjustKind::SelectiveColor) && selective_group == 0,
        ),
    ] {
        if widgets::tab_bar_state(tree, bar)?.is_disabled() == tabs_live {
            widgets::set_tab_bar_disabled(tree, bar, !tabs_live)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// Which editor group is shown (the active layer's kind as of the last
/// [`sync_adjust_controls`]), `None` when the column is hidden.
#[must_use]
pub fn adjust_controls_shown(
    tree: &WidgetTree<WidgetKind>,
    controls: AdjustControls,
) -> Option<AdjustKind> {
    if tree
        .style(controls.root)
        .is_none_or(|style| style.display == Display::None)
    {
        return None;
    }
    AdjustKind::ALL.into_iter().find(|kind| {
        controls
            .groups
            .get(kind.index())
            .and_then(|&group| tree.style(group))
            .is_some_and(|style| style.display != Display::None)
    })
}

fn checkbox_checked(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    matches!(
        tree.payload(id),
        Some(WidgetKind::Checkbox(state)) if state.checked == accesskit::Toggled::True
    )
}

/// Whether Photo Filter's Preserve Luminosity checkbox is checked.
#[must_use]
pub fn adjust_preserve_checked(tree: &WidgetTree<WidgetKind>, controls: AdjustControls) -> bool {
    checkbox_checked(tree, controls.preserve)
}

/// Whether Black & White's Tint checkbox is checked.
#[must_use]
pub fn adjust_tint_checked(tree: &WidgetTree<WidgetKind>, controls: AdjustControls) -> bool {
    checkbox_checked(tree, controls.tint)
}

/// Whether Color Balance's Preserve Luminosity checkbox is checked.
#[must_use]
pub fn adjust_balance_preserve_checked(
    tree: &WidgetTree<WidgetKind>,
    controls: AdjustControls,
) -> bool {
    checkbox_checked(tree, controls.balance_preserve)
}

/// Whether Brightness/Contrast's Use Legacy checkbox is checked.
#[must_use]
pub fn adjust_legacy_checked(tree: &WidgetTree<WidgetKind>, controls: AdjustControls) -> bool {
    checkbox_checked(tree, controls.legacy)
}

/// The Color Balance range the three sliders edit: `0` shadows, `1`
/// midtones, `2` highlights (the selector's selected tab; `0` if it cannot
/// be read).
#[must_use]
pub fn adjust_balance_range(tree: &WidgetTree<WidgetKind>, controls: AdjustControls) -> usize {
    widgets::tab_bar_state(tree, controls.balance_range)
        .map_or(0, aurora_widgets::widgets::TabBarState::selected)
}

/// Whether Channel Mixer's Monochrome checkbox is checked.
#[must_use]
pub fn adjust_mixer_mono_checked(tree: &WidgetTree<WidgetKind>, controls: AdjustControls) -> bool {
    checkbox_checked(tree, controls.mixer_mono)
}

/// Whether Selective Color's Absolute checkbox is checked.
#[must_use]
pub fn adjust_selective_absolute_checked(
    tree: &WidgetTree<WidgetKind>,
    controls: AdjustControls,
) -> bool {
    checkbox_checked(tree, controls.selective_absolute)
}

/// Whether Gradient Map's Reverse checkbox is checked.
#[must_use]
pub fn adjust_gradient_reverse_checked(
    tree: &WidgetTree<WidgetKind>,
    controls: AdjustControls,
) -> bool {
    checkbox_checked(tree, controls.gradient_reverse)
}

/// The summary row's text for a gradient: its stop count, and whether the
/// colours can be edited here.
#[must_use]
pub fn gradient_summary_text(stops: usize) -> String {
    match stops {
        2 => "2 colour stops".to_owned(),
        n => format!("{n} colour stops (only two-stop gradients are editable here)"),
    }
}

/// The tab of the selector that belongs to a slider of `kind`, the `range`
/// [`AdjustSlider::value_of`] and [`AdjustSlider::with_value`] take: Color
/// Balance's range, Channel Mixer's output, Selective Color's range; `0` for
/// every other kind.
#[must_use]
pub fn adjust_tab(
    tree: &WidgetTree<WidgetKind>,
    controls: AdjustControls,
    kind: AdjustKind,
) -> usize {
    let bar = match kind {
        AdjustKind::ColorBalance => controls.balance_range,
        AdjustKind::ChannelMixer => controls.mixer_output,
        AdjustKind::SelectiveColor => {
            // Group 0 holds the six colour ranges (the second bar picks
            // one); groups 1 to 3 are Whites, Neutrals and Blacks.
            let group = widgets::tab_bar_state(tree, controls.selective_group)
                .map_or(0, aurora_widgets::widgets::TabBarState::selected);
            if group > 0 {
                return 5 + group;
            }
            controls.selective_range
        }
        _ => return 0,
    };
    widgets::tab_bar_state(tree, bar).map_or(0, aurora_widgets::widgets::TabBarState::selected)
}

/// Sets slider `id` to `value`, also while it is disabled (a tint slider
/// with Tint off still shows the stored colour); it stays disabled.
fn put_slider_value(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    value: f64,
) -> Result<(), WidgetError> {
    let disabled = matches!(
        tree.payload(id),
        Some(WidgetKind::Slider(state)) if state.disabled
    );
    if disabled {
        widgets::set_slider_disabled(tree, id, false)?;
    }
    let set = widgets::set_slider_value(tree, id, value);
    if disabled {
        widgets::set_slider_disabled(tree, id, true)?;
    }
    set.map(|_| ())
}

/// Sets `id`'s checkbox to `on`; returns whether it changed.
fn sync_checkbox(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    on: bool,
) -> Result<bool, WidgetError> {
    if checkbox_checked(tree, id) == on {
        return Ok(false);
    }
    let checked = if on {
        accesskit::Toggled::True
    } else {
        accesskit::Toggled::False
    };
    widgets::set_checkbox_checked(tree, id, checked)?;
    Ok(true)
}

/// Mirrors `active` into `controls`, in place — never inserting or
/// removing a widget. Returns whether anything changed (cheap enough to
/// run every event-loop iteration).
///
/// - `active` not one of the editors' adjustments (or `None`): hidden and
///   disabled.
/// - Otherwise that kind's group is shown; every slider of it shows the
///   document's value (a slider equal to `captured` is left alone: mid-drag
///   it is the source of truth; the Color Balance sliders show the range
///   the selector holds) and every readout follows.
///
/// # Errors
///
/// Whatever the underlying widget setters return for an id that isn't the
/// widget kind `controls` says it is.
pub fn sync_adjust_controls(
    tree: &mut WidgetTree<WidgetKind>,
    controls: AdjustControls,
    layers: &LayerTree,
    active: Option<LayerId>,
    captured: Option<WidgetId>,
) -> Result<bool, WidgetError> {
    let Some(adjustment) = active.and_then(|id| adjust_of(layers, id)) else {
        return set_shown(tree, controls, None, false, false, false);
    };
    let kind = AdjustKind::of(adjustment);
    let tint_on = adjustment.black_white().is_some_and(|p| p.tint());
    let mixer_mono = adjustment.channel_mixer().is_some_and(|p| p.monochrome());
    let gradient_colours = adjustment
        .gradient_map()
        .is_some_and(aurora_core::GradientMapParams::is_two_stop);
    let mut changed = set_shown(tree, controls, kind, tint_on, mixer_mono, gradient_colours)?;
    if let Some(filter) = adjustment.photo_filter() {
        changed |= sync_checkbox(tree, controls.preserve, filter.preserve_luminosity())?;
    }
    if let Some(bw) = adjustment.black_white() {
        changed |= sync_checkbox(tree, controls.tint, bw.tint())?;
    }
    if let Some(balance) = adjustment.color_balance() {
        changed |= sync_checkbox(
            tree,
            controls.balance_preserve,
            balance.preserve_luminosity(),
        )?;
    }
    if let Some(bc) = adjustment.brightness_contrast() {
        changed |= sync_checkbox(tree, controls.legacy, bc.legacy())?;
    }
    if let Some(mixer) = adjustment.channel_mixer() {
        changed |= sync_checkbox(tree, controls.mixer_mono, mixer.monochrome())?;
    }
    if let Some(selective) = adjustment.selective_color() {
        changed |= sync_checkbox(tree, controls.selective_absolute, selective.absolute())?;
    }
    if let Some(gradient) = adjustment.gradient_map() {
        changed |= sync_checkbox(tree, controls.gradient_reverse, gradient.reverse())?;
        changed |= widgets::set_label_text(
            tree,
            controls.gradient_summary,
            &gradient_summary_text(gradient.stops().len()),
        )?;
    }
    for slider in AdjustSlider::ALL
        .into_iter()
        .filter(|s| Some(s.kind()) == kind)
    {
        let range = adjust_tab(tree, controls, slider.kind());
        let (Some(&id), Some(&readout)) = (
            controls.sliders.get(slider.index()),
            controls.readouts.get(slider.index()),
        ) else {
            continue;
        };
        let Some(value) = slider.value_of(adjustment, range) else {
            continue;
        };
        // Brightness/Contrast: the slider's range follows the layer's mode.
        if let Some(bc) = adjustment.brightness_contrast() {
            let ((b_lo, b_hi), (c_lo, c_hi)) = BrightnessContrastParams::ranges(bc.legacy());
            let (lo, hi) = match slider {
                AdjustSlider::Brightness => (b_lo, b_hi),
                _ => (c_lo, c_hi),
            };
            changed |= widgets::set_slider_range(tree, id, f64::from(lo), f64::from(hi))?;
        }
        if captured != Some(id) {
            let shown = match tree.payload(id) {
                Some(WidgetKind::Slider(state)) => state.value,
                _ => return Err(WidgetError::WrongWidgetKind(id)),
            };
            if shown.to_bits() != value.to_bits() {
                put_slider_value(tree, id, value)?;
                changed = true;
            }
        }
        let shown_value = match tree.payload(id) {
            Some(WidgetKind::Slider(state)) => slider.snap(state.value),
            _ => value,
        };
        let text = readout_for(slider, shown_value, adjustment, range);
        changed |= widgets::set_label_text(tree, readout, &text)?;
    }
    Ok(changed)
}

/// The readout text of `slider` at `value`: the slider's own for most; for
/// Channel Mixer and Selective Color it names the output or range the
/// selector holds ("Green output, Red: +50", "Blues, Cyan: -20"), because
/// those selectors' tabs are short.
fn readout_for(slider: AdjustSlider, value: f64, adjustment: &Adjustment, tab: usize) -> String {
    let plain = slider.readout(value);
    let scope = match (slider.kind(), adjustment.channel_mixer()) {
        (AdjustKind::ChannelMixer, Some(p)) if p.monochrome() => Some("Gray".to_owned()),
        (AdjustKind::ChannelMixer, _) => CHANNEL_MIXER_OUTPUTS
            .get(tab)
            .map(|name| format!("{name} output")),
        (AdjustKind::SelectiveColor, _) => SELECTIVE_COLOR_RANGES.get(tab).map(|n| (*n).to_owned()),
        _ => None,
    };
    match scope {
        Some(scope) => format!("{scope}, {plain}"),
        None => plain,
    }
}

/// Whether `id` is (or lies inside) the editors' column.
#[must_use]
pub fn adjust_controls_contains(
    tree: &WidgetTree<WidgetKind>,
    controls: &AdjustControls,
    id: WidgetId,
) -> bool {
    tree.is_within(controls.root, id)
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::redundant_closure_for_method_calls,
    clippy::useless_conversion,
    clippy::map_unwrap_or
)]
mod tests {
    use super::{
        AdjustKind, AdjustSlider as S, adjust_controls_shown, adjust_preserve_checked,
        insert_adjust_controls, sync_adjust_controls,
    };
    use aurora_core::{ExposureParams, PhotoFilterParams, VibranceParams};
    use aurora_doc::{Adjustment, LayerTree};
    use aurora_widgets::widgets::{self, WidgetKind};

    fn scales() -> aurora_theme::Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn slider_state(
        tree: &aurora_widgets::WidgetTree<WidgetKind>,
        id: aurora_widgets::WidgetId,
    ) -> (f64, bool) {
        match tree.payload(id) {
            Some(WidgetKind::Slider(state)) => (state.value, state.disabled),
            other => unreachable!("{other:?}"),
        }
    }

    fn vib(v: f32, s: f32) -> Adjustment {
        match VibranceParams::new(v, s) {
            Ok(p) => Adjustment::Vibrance(p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn exp(e: f32, o: f32, g: f32) -> Adjustment {
        match ExposureParams::new(e, o, g) {
            Ok(p) => Adjustment::Exposure(p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn filter(colour: [f32; 3], d: f32, p: bool) -> Adjustment {
        match PhotoFilterParams::new(colour, d, p) {
            Ok(p) => Adjustment::PhotoFilter(p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn add(layers: &mut LayerTree, name: &str, adjustment: Adjustment) -> aurora_doc::LayerId {
        let Ok(id) = layers.add_adjustment_layer_at(name, adjustment, None, 0) else {
            unreachable!("adjustment layer");
        };
        id
    }

    fn show(
        ws: &mut crate::Workspace,
        controls: super::AdjustControls,
        layers: &LayerTree,
        id: Option<aurora_doc::LayerId>,
    ) -> Result<bool, aurora_widgets::WidgetError> {
        sync_adjust_controls(&mut ws.tree, controls, layers, id, None)
    }

    #[test]
    fn the_editor_shows_the_group_of_the_active_layer_kind() {
        let scales = scales();
        let mut ws = crate::build_workspace(&scales);
        let controls = match insert_adjust_controls(&mut ws.tree, ws.properties.root, &scales) {
            Ok(c) => c,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(adjust_controls_shown(&ws.tree, controls), None);
        let mut layers = LayerTree::new();
        let v = add(&mut layers, "V", vib(35.0, -12.0));
        let e = add(&mut layers, "E", exp(-1.25, 0.125, 2.2));
        let f = add(&mut layers, "F", filter([0.0, 0.5, 1.0], 61.0, false));
        let hs = add(
            &mut layers,
            "H",
            Adjustment::HueSaturation(aurora_core::HueSaturationParams::identity()),
        );
        assert_eq!(show(&mut ws, controls, &layers, None).ok(), Some(false));
        assert_eq!(show(&mut ws, controls, &layers, Some(v)).ok(), Some(true));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::Vibrance)
        );
        let at = |ws: &crate::Workspace, s: S| slider_state(&ws.tree, controls.sliders[s.index()]);
        assert_eq!(at(&ws, S::Vibrance), (35.0, false));
        assert_eq!(at(&ws, S::VibranceSaturation), (-12.0, false));
        assert!(
            at(&ws, S::Exposure).1 && at(&ws, S::Red).1,
            "other groups disabled"
        );
        match ws
            .tree
            .payload(controls.readouts[S::VibranceSaturation.index()])
        {
            Some(WidgetKind::Label(label)) => assert_eq!(label.text, "Saturation: -12"),
            other => unreachable!("{other:?}"),
        }
        let _ = show(&mut ws, controls, &layers, Some(e));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::Exposure)
        );
        assert_eq!(at(&ws, S::Exposure), (-1.25, false));
        assert_eq!(at(&ws, S::Offset), (0.125, false));
        assert!((at(&ws, S::Gamma).0 - 2.2).abs() < 1e-6);
        assert!(at(&ws, S::Vibrance).1);
        match ws.tree.payload(controls.readouts[S::Exposure.index()]) {
            Some(WidgetKind::Label(label)) => assert_eq!(label.text, "Exposure: -1.25"),
            other => unreachable!("{other:?}"),
        }
        match ws.tree.payload(controls.readouts[S::Offset.index()]) {
            Some(WidgetKind::Label(label)) => assert_eq!(label.text, "Offset: 0.125"),
            other => unreachable!("{other:?}"),
        }
        let _ = show(&mut ws, controls, &layers, Some(f));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::PhotoFilter)
        );
        assert_eq!(at(&ws, S::Density), (61.0, false));
        assert_eq!(at(&ws, S::Red), (0.0, false));
        assert_eq!(at(&ws, S::Green), (128.0, false));
        assert_eq!(at(&ws, S::Blue), (255.0, false));
        assert!(!adjust_preserve_checked(&ws.tree, controls));
        // The checkbox follows the document.
        let _ = layers.set_adjustment(f, filter([0.0, 0.5, 1.0], 61.0, true));
        let _ = show(&mut ws, controls, &layers, Some(f));
        assert!(adjust_preserve_checked(&ws.tree, controls));
        // A captured slider keeps its mid-drag value; released, the document wins.
        let density = controls.sliders[S::Density.index()];
        let _ = widgets::set_slider_value(&mut ws.tree, density, 80.0);
        let _ = sync_adjust_controls(&mut ws.tree, controls, &layers, Some(f), Some(density));
        assert_eq!(slider_state(&ws.tree, density).0, 80.0);
        let _ = show(&mut ws, controls, &layers, Some(f));
        assert_eq!(slider_state(&ws.tree, density).0, 61.0);
        // Another adjustment kind hides it all.
        assert_eq!(show(&mut ws, controls, &layers, Some(hs)).ok(), Some(true));
        assert_eq!(adjust_controls_shown(&ws.tree, controls), None);
        assert!(at(&ws, S::Density).1);
    }

    #[test]
    fn slider_values_snap_to_their_decimals_and_clamp() {
        let v = vib(0.0, 0.0);
        let e = exp(0.0, 0.0, 1.0);
        let f = filter([1.0, 1.0, 1.0], 25.0, true);
        let bw = Adjustment::BlackWhite(aurora_core::BlackWhiteParams::default());
        let cb = Adjustment::ColorBalance(aurora_core::ColorBalanceParams::identity());
        let th = Adjustment::Threshold(aurora_core::ThresholdParams::default());
        let po = Adjustment::Posterize(aurora_core::PosterizeParams::default());
        let bc = Adjustment::BrightnessContrast(aurora_core::BrightnessContrastParams::identity());
        let cm = Adjustment::ChannelMixer(aurora_core::ChannelMixerParams::identity());
        let sc = Adjustment::SelectiveColor(aurora_core::SelectiveColorParams::identity());
        let gm = Adjustment::GradientMap(aurora_core::GradientMapParams::black_to_white());
        for s in S::ALL {
            let base = match s.kind() {
                AdjustKind::Vibrance => &v,
                AdjustKind::Exposure => &e,
                AdjustKind::PhotoFilter => &f,
                AdjustKind::BlackWhite => &bw,
                AdjustKind::ColorBalance => &cb,
                AdjustKind::Invert => continue,
                AdjustKind::Threshold => &th,
                AdjustKind::Posterize => &po,
                AdjustKind::BrightnessContrast => &bc,
                AdjustKind::ChannelMixer => &cm,
                AdjustKind::SelectiveColor => &sc,
                AdjustKind::GradientMap => &gm,
            };
            let (mut min, max) = s.range();
            if s == S::Contrast {
                // The slider spans both modes; the default mode stops at -50.
                min = f64::from(aurora_core::CONTRAST_MIN);
            }
            let lo = s
                .with_value(base, 0, min - 50.0)
                .and_then(|a| s.value_of(&a, 0));
            let hi = s
                .with_value(base, 0, max + 50.0)
                .and_then(|a| s.value_of(&a, 0));
            assert!((lo.unwrap_or(f64::NAN) - min).abs() < 1e-6, "{s:?} {lo:?}");
            assert!((hi.unwrap_or(f64::NAN) - max).abs() < 1e-6, "{s:?} {hi:?}");
            assert!(s.with_value(base, 0, f64::NAN).is_none());
            assert!(s.with_value(base, 0, f64::INFINITY).is_none());
        }
        // The wrong kind of adjustment gives nothing.
        assert!(S::Vibrance.with_value(&e, 0, 5.0).is_none());
        assert!(S::Gamma.with_value(&f, 0, 2.0).is_none());
        assert!(S::Red.with_value(&v, 0, 2.0).is_none());
        // Rounding.
        let got = |s: S, base: &Adjustment, x: f64| {
            s.with_value(base, 0, x).and_then(|a| s.value_of(&a, 0))
        };
        assert_eq!(got(S::Vibrance, &v, 40.6), Some(41.0));
        assert!((got(S::Exposure, &e, 1.2349).unwrap_or(0.0) - 1.23).abs() < 1e-6);
        assert!((got(S::Offset, &e, 0.12349).unwrap_or(0.0) - 0.123).abs() < 1e-6);
        assert!((got(S::Gamma, &e, 2.226).unwrap_or(0.0) - 2.23).abs() < 1e-6);
        assert_eq!(got(S::Density, &f, 33.4), Some(33.0));
        assert_eq!(got(S::Green, &f, 138.4), Some(138.0));
    }

    #[test]
    fn a_slider_edit_changes_only_its_own_field() {
        let e = exp(1.0, 0.25, 2.0);
        let next = S::Offset.with_value(&e, 0, -0.25);
        let p = next.as_ref().and_then(Adjustment::exposure);
        assert_eq!(
            p.map(|p| (p.exposure(), p.offset(), p.gamma())),
            Some((1.0, -0.25, 2.0))
        );
        let f = filter([0.2, 0.4, 0.6], 30.0, false);
        let next = S::Blue.with_value(&f, 0, 255.0);
        let p = next.as_ref().and_then(Adjustment::photo_filter);
        assert_eq!(
            p.map(|p| (p.colour()[0], p.colour()[1], p.colour()[2], p.density())),
            Some((0.2, 0.4, 1.0, 30.0))
        );
        assert_eq!(p.map(|p| p.preserve_luminosity()), Some(false));
        let v = vib(10.0, 20.0);
        let next = S::VibranceSaturation.with_value(&v, 0, -30.0);
        let p = next.as_ref().and_then(Adjustment::vibrance);
        assert_eq!(
            p.map(|p| (p.vibrance(), p.saturation())),
            Some((10.0, -30.0))
        );
    }

    #[test]
    fn the_preset_colours_survive_the_8_bit_sliders_exactly() {
        for (_, [r, g, b]) in aurora_core::PHOTO_FILTER_PRESETS {
            let colour = [r, g, b].map(|c| f32::from(c) / 255.0);
            let f = filter(colour, 25.0, true);
            let seen = [S::Red, S::Green, S::Blue].map(|s| s.value_of(&f, 0));
            assert_eq!(seen, [r, g, b].map(|c| Some(f64::from(c))));
            // And re-applying what the slider shows changes nothing.
            let again = S::Green.with_value(&f, 0, seen[1].unwrap_or(0.0));
            assert_eq!(again, Some(f));
        }
    }

    fn bw(weights: [f32; 6], tint: bool, hue: f32, sat: f32) -> Adjustment {
        match aurora_core::BlackWhiteParams::new(weights, tint, hue, sat) {
            Ok(p) => Adjustment::BlackWhite(p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn cb(ranges: [[f32; 3]; 3], preserve: bool) -> Adjustment {
        match aurora_core::ColorBalanceParams::new(ranges, preserve) {
            Ok(p) => Adjustment::ColorBalance(p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn built(ws: &mut crate::Workspace) -> super::AdjustControls {
        match insert_adjust_controls(&mut ws.tree, ws.properties.root, &scales()) {
            Ok(c) => c,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn readout(ws: &crate::Workspace, controls: super::AdjustControls, s: S) -> String {
        match ws.tree.payload(controls.readouts[s.index()]) {
            Some(WidgetKind::Label(label)) => label.text.clone(),
            other => unreachable!("{other:?}"),
        }
    }

    /// 0.240.0: the Black & White group mirrors its layer; the two tint
    /// sliders are live only while Tint is on and follow the document.
    #[test]
    fn the_black_and_white_group_follows_its_layer_and_tint_gates_two_sliders() {
        let mut ws = crate::build_workspace(&scales());
        let controls = built(&mut ws);
        let mut layers = LayerTree::new();
        let off = add(
            &mut layers,
            "off",
            bw([-200.0, 12.0, 300.0, 0.0, 99.0, 1.0], false, 211.0, 64.0),
        );
        let on = add(
            &mut layers,
            "on",
            bw([40.0, 60.0, 40.0, 60.0, 20.0, 80.0], true, 35.0, 25.0),
        );
        let at = |ws: &crate::Workspace, s: S| slider_state(&ws.tree, controls.sliders[s.index()]);
        assert_eq!(show(&mut ws, controls, &layers, Some(off)).ok(), Some(true));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::BlackWhite)
        );
        for (s, want) in [
            (S::BwReds, -200.0),
            (S::BwYellows, 12.0),
            (S::BwGreens, 300.0),
            (S::BwCyans, 0.0),
            (S::BwBlues, 99.0),
            (S::BwMagentas, 1.0),
        ] {
            assert_eq!(at(&ws, s), (want, false), "{s:?}");
        }
        assert_eq!(readout(&ws, controls, S::BwReds), "Reds: -200");
        assert_eq!(readout(&ws, controls, S::BwMagentas), "Magentas: 1");
        // Tint off: the tint sliders show the stored colour but are disabled.
        assert_eq!(at(&ws, S::TintHue), (211.0, true));
        assert_eq!(at(&ws, S::TintSaturation), (64.0, true));
        assert!(!super::adjust_tint_checked(&ws.tree, controls));
        // Other groups stay disabled.
        assert!(at(&ws, S::Vibrance).1 && at(&ws, S::BalanceCyanRed).1 && at(&ws, S::Red).1);
        let _ = show(&mut ws, controls, &layers, Some(on));
        assert!(super::adjust_tint_checked(&ws.tree, controls));
        assert_eq!(at(&ws, S::TintHue), (35.0, false));
        assert_eq!(at(&ws, S::TintSaturation), (25.0, false));
        assert_eq!(readout(&ws, controls, S::TintHue), "Tint hue: 35");
        // The document changing the flag re-gates the sliders.
        let _ = layers.set_adjustment(
            on,
            bw([40.0, 60.0, 40.0, 60.0, 20.0, 80.0], false, 35.0, 25.0),
        );
        let _ = show(&mut ws, controls, &layers, Some(on));
        assert!(!super::adjust_tint_checked(&ws.tree, controls));
        assert!(at(&ws, S::TintHue).1 && at(&ws, S::TintSaturation).1);
        assert!(!at(&ws, S::BwReds).1, "the colour sliders stay live");
    }

    /// The Color Balance range selector rebinds the three sliders to a
    /// range without touching the document; an edit changes one range only.
    #[test]
    fn the_color_balance_range_selector_rebinds_the_sliders_without_a_document_change() {
        let mut ws = crate::build_workspace(&scales());
        let controls = built(&mut ws);
        let ranges = [[1.0, -2.0, 3.0], [-40.0, 50.0, -60.0], [70.0, 80.0, -90.0]];
        let mut layers = LayerTree::new();
        let id = add(&mut layers, "cb", cb(ranges, false));
        let at = |ws: &crate::Workspace, s: S| slider_state(&ws.tree, controls.sliders[s.index()]);
        let _ = show(&mut ws, controls, &layers, Some(id));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::ColorBalance)
        );
        assert_eq!(super::adjust_balance_range(&ws.tree, controls), 0);
        assert!(!super::adjust_balance_preserve_checked(&ws.tree, controls));
        assert!(
            widgets::tab_bar_state(&ws.tree, controls.balance_range)
                .is_ok_and(|t| !t.is_disabled())
        );
        assert_eq!(at(&ws, S::BalanceCyanRed), (1.0, false));
        assert_eq!(readout(&ws, controls, S::BalanceCyanRed), "Cyan-Red: +1");
        assert_eq!(
            readout(&ws, controls, S::BalanceMagentaGreen),
            "Magenta-Green: -2"
        );
        let before = layers.adjustment(id).cloned();
        for (tab, row) in ranges.iter().enumerate() {
            let _ = widgets::select_tab(&mut ws.tree, controls.balance_range, tab);
            let _ = show(&mut ws, controls, &layers, Some(id));
            assert_eq!(super::adjust_balance_range(&ws.tree, controls), tab);
            for (slot, s) in [
                S::BalanceCyanRed,
                S::BalanceMagentaGreen,
                S::BalanceYellowBlue,
            ]
            .into_iter()
            .enumerate()
            {
                assert_eq!(at(&ws, s), (f64::from(row[slot]), false), "tab {tab} {s:?}");
            }
        }
        assert_eq!(
            layers.adjustment(id).cloned(),
            before,
            "selecting a tab edits nothing"
        );
        // An edit on the highlights tab changes the highlights only.
        let current = layers
            .adjustment(id)
            .cloned()
            .unwrap_or_else(|| unreachable!("layer"));
        let next = S::BalanceYellowBlue.with_value(&current, 2, 33.4);
        let p = next.as_ref().and_then(Adjustment::color_balance);
        assert_eq!(
            p.map(|p| p.ranges()),
            Some([[1.0, -2.0, 3.0], [-40.0, 50.0, -60.0], [70.0, 80.0, 33.0]])
        );
        assert_eq!(p.map(|p| p.preserve_luminosity()), Some(false));
        // The same slider on another range is a different field.
        let other = S::BalanceYellowBlue.with_value(&current, 0, 33.4);
        assert_eq!(
            other
                .as_ref()
                .and_then(Adjustment::color_balance)
                .map(|p| p.ranges()),
            Some([[1.0, -2.0, 33.0], [-40.0, 50.0, -60.0], [70.0, 80.0, -90.0]])
        );
        assert_eq!(S::BalanceYellowBlue.value_of(&current, 1), Some(-60.0));
        assert_eq!(
            S::BalanceYellowBlue.value_of(&current, 9),
            None,
            "no such range"
        );
        assert!(S::BalanceYellowBlue.with_value(&current, 9, 1.0).is_none());
        // The checkbox follows the document.
        let _ = layers.set_adjustment(id, cb(ranges, true));
        let _ = show(&mut ws, controls, &layers, Some(id));
        assert!(super::adjust_balance_preserve_checked(&ws.tree, controls));
        // Another group disables the selector and the checkbox.
        let v = add(&mut layers, "v", vib(0.0, 0.0));
        let _ = show(&mut ws, controls, &layers, Some(v));
        assert!(
            widgets::tab_bar_state(&ws.tree, controls.balance_range)
                .is_ok_and(aurora_widgets::widgets::TabBarState::is_disabled)
        );
        assert!(at(&ws, S::BalanceCyanRed).1);
    }

    #[test]
    fn black_and_white_edits_change_only_their_own_field() {
        let w = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0];
        let base = bw(w, true, 100.0, 40.0);
        for (slot, s) in [
            S::BwReds,
            S::BwYellows,
            S::BwGreens,
            S::BwCyans,
            S::BwBlues,
            S::BwMagentas,
        ]
        .into_iter()
        .enumerate()
        {
            let next = s.with_value(&base, 0, 275.4);
            let p = next.as_ref().and_then(Adjustment::black_white);
            let mut want = w;
            want[slot] = 275.0;
            assert_eq!(p.map(|p| p.weights()), Some(want), "{s:?}");
            assert_eq!(
                p.map(|p| (p.tint(), p.tint_hue(), p.tint_saturation())),
                Some((true, 100.0, 40.0))
            );
        }
        let hue = S::TintHue
            .with_value(&base, 0, 359.6)
            .and_then(|a| a.black_white());
        assert_eq!(
            hue.map(|p| (p.tint_hue(), p.tint_saturation(), p.weights())),
            Some((360.0, 40.0, w))
        );
        let sat = S::TintSaturation
            .with_value(&base, 0, -5.0)
            .and_then(|a| a.black_white());
        assert_eq!(
            sat.map(|p| (p.tint_hue(), p.tint_saturation())),
            Some((100.0, 0.0))
        );
        // Wrong adjustment: nothing.
        assert!(S::BwReds.with_value(&vib(0.0, 0.0), 0, 1.0).is_none());
        assert!(S::BalanceCyanRed.with_value(&base, 0, 1.0).is_none());
    }

    /// Every slider's accessible name is unique and non-empty; the widgets
    /// the editors build carry them; the two Preserve Luminosity checkboxes
    /// sit in different groups, never shown together.
    #[test]
    fn accessible_names_are_unique_among_the_sliders() {
        let mut names: Vec<&str> = S::ALL.iter().map(|s| s.label()).collect();
        assert!(names.iter().all(|n| !n.is_empty()));
        names.sort_unstable();
        let total = names.len();
        names.dedup();
        assert_eq!(names.len(), total, "{names:?}");
        let mut ws = crate::build_workspace(&scales());
        let controls = built(&mut ws);
        for s in S::ALL {
            let id = controls.sliders[s.index()];
            let node = ws.tree.accessibility(id);
            assert_eq!(
                node.and_then(accesskit::Node::label),
                Some(s.label()),
                "{s:?}"
            );
        }
        let tabs = widgets::tab_bar_state(&ws.tree, controls.balance_range);
        assert!(tabs.is_ok_and(|t| t.labels().len() == 3 && t.accessible_name(2) == "Highlights"));
    }

    #[test]
    fn the_new_groups_hide_for_other_layers_and_every_slider_is_in_its_group() {
        let mut ws = crate::build_workspace(&scales());
        let controls = built(&mut ws);
        let mut layers = LayerTree::new();
        let id = add(&mut layers, "b", bw([1.0; 6], false, 0.0, 0.0));
        let hs = add(
            &mut layers,
            "H",
            Adjustment::HueSaturation(aurora_core::HueSaturationParams::identity()),
        );
        let _ = show(&mut ws, controls, &layers, Some(id));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::BlackWhite)
        );
        assert_eq!(show(&mut ws, controls, &layers, Some(hs)).ok(), Some(true));
        assert_eq!(adjust_controls_shown(&ws.tree, controls), None);
        for s in S::ALL {
            assert!(
                slider_state(&ws.tree, controls.sliders[s.index()]).1,
                "{s:?} disabled"
            );
            let group = controls.groups[s.kind().index()];
            assert!(
                ws.tree.is_within(group, controls.sliders[s.index()]),
                "{s:?}"
            );
            assert!(
                ws.tree.is_within(group, controls.readouts[s.index()]),
                "{s:?}"
            );
        }
        assert!(ws.tree.is_within(
            controls.groups[AdjustKind::BlackWhite.index()],
            controls.tint
        ));
        assert!(ws.tree.is_within(
            controls.groups[AdjustKind::ColorBalance.index()],
            controls.balance_preserve
        ));
        assert!(ws.tree.is_within(
            controls.groups[AdjustKind::ColorBalance.index()],
            controls.balance_range
        ));
        assert!(ws.tree.is_within(
            controls.groups[AdjustKind::PhotoFilter.index()],
            controls.preserve
        ));
        assert_eq!(S::ALL.len(), super::ADJUST_SLIDER_COUNT);
        for (i, s) in S::ALL.into_iter().enumerate() {
            assert_eq!(s.index(), i);
        }
        for (i, k) in AdjustKind::ALL.into_iter().enumerate() {
            assert_eq!(k.index(), i);
        }
    }

    /// 0.244.0: the four new kinds' groups, sliders and snapping.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn threshold_posterize_brightness_contrast_and_invert_editors_follow_the_document() {
        let scales = scales();
        let mut ws = crate::build_workspace(&scales);
        let controls = match insert_adjust_controls(&mut ws.tree, ws.properties.root, &scales) {
            Ok(c) => c,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut layers = LayerTree::new();
        let t = add(
            &mut layers,
            "T",
            Adjustment::Threshold(match aurora_core::ThresholdParams::new(172) {
                Ok(p) => p,
                Err(err) => unreachable!("{err:?}"),
            }),
        );
        let p = add(
            &mut layers,
            "P",
            Adjustment::Posterize(match aurora_core::PosterizeParams::new(13) {
                Ok(p) => p,
                Err(err) => unreachable!("{err:?}"),
            }),
        );
        let b = add(
            &mut layers,
            "B",
            Adjustment::BrightnessContrast(
                match aurora_core::BrightnessContrastParams::new(-95, -50, false) {
                    Ok(p) => p,
                    Err(err) => unreachable!("{err:?}"),
                },
            ),
        );
        let i = add(&mut layers, "I", Adjustment::Invert);
        let at = |ws: &crate::Workspace, s: S| slider_state(&ws.tree, controls.sliders[s.index()]);
        let _ = show(&mut ws, controls, &layers, Some(t));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::Threshold)
        );
        assert_eq!(at(&ws, S::ThresholdLevel), (172.0, false));
        assert!(at(&ws, S::PosterizeLevels).1 && at(&ws, S::Brightness).1);
        let _ = show(&mut ws, controls, &layers, Some(p));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::Posterize)
        );
        assert_eq!(at(&ws, S::PosterizeLevels), (13.0, false));
        assert!(at(&ws, S::ThresholdLevel).1);
        let _ = show(&mut ws, controls, &layers, Some(b));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::BrightnessContrast)
        );
        assert_eq!(at(&ws, S::Brightness), (-95.0, false));
        assert_eq!(at(&ws, S::Contrast), (-50.0, false));
        assert!(!crate::adjust_legacy_checked(&ws.tree, controls));
        match ws.tree.payload(controls.readouts[S::Brightness.index()]) {
            Some(WidgetKind::Label(label)) => assert_eq!(label.text, "Brightness: -95"),
            other => unreachable!("{other:?}"),
        }
        let _ = show(&mut ws, controls, &layers, Some(i));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::Invert)
        );
        assert!(at(&ws, S::Brightness).1 && at(&ws, S::ThresholdLevel).1);
        // `with_value` snaps, clamps to the slider and to the mode's range.
        let adj = |id| layers.adjustment(id).cloned().unwrap_or(Adjustment::Invert);
        assert_eq!(
            S::ThresholdLevel
                .with_value(&adj(t), 0, 300.0)
                .and_then(|a| a.threshold())
                .map(|p| p.level()),
            Some(255)
        );
        assert_eq!(
            S::ThresholdLevel
                .with_value(&adj(t), 0, -4.0)
                .and_then(|a| a.threshold())
                .map(|p| p.level()),
            Some(1)
        );
        assert_eq!(
            S::PosterizeLevels
                .with_value(&adj(p), 0, 0.4)
                .and_then(|a| a.posterize())
                .map(|p| p.levels()),
            Some(2)
        );
        assert!(
            S::ThresholdLevel.with_value(&adj(p), 0, 5.0).is_none(),
            "another kind"
        );
        assert!(S::Contrast.with_value(&adj(b), 0, f64::NAN).is_none());
        let legacy = Adjustment::BrightnessContrast(
            match aurora_core::BrightnessContrastParams::new(0, 0, true) {
                Ok(p) => p,
                Err(err) => unreachable!("{err:?}"),
            },
        );
        let got = |slider: S, value| {
            slider
                .with_value(&legacy, 0, value)
                .and_then(|a| a.brightness_contrast())
                .map(|p| (p.brightness(), p.contrast()))
        };
        assert_eq!(got(S::Brightness, 150.0), Some((100, 0)));
        assert_eq!(got(S::Contrast, -100.0), Some((0, -100)));
        let default =
            Adjustment::BrightnessContrast(aurora_core::BrightnessContrastParams::identity());
        assert_eq!(
            S::Contrast
                .with_value(&default, 0, -100.0)
                .and_then(|a| a.brightness_contrast())
                .map(|p| p.contrast()),
            Some(-50)
        );
    }

    /// 0.244.0: the Brightness and Contrast sliders' ranges follow the
    /// layer's mode when synced, and an out-of-range displayed value is
    /// clamped into the new range.
    #[test]
    fn the_brightness_contrast_slider_ranges_follow_the_layers_mode() {
        let scales = scales();
        let mut ws = crate::build_workspace(&scales);
        let controls = match insert_adjust_controls(&mut ws.tree, ws.properties.root, &scales) {
            Ok(c) => c,
            Err(err) => unreachable!("{err:?}"),
        };
        let bc = |b, c, legacy| match aurora_core::BrightnessContrastParams::new(b, c, legacy) {
            Ok(p) => Adjustment::BrightnessContrast(p),
            Err(err) => unreachable!("{err:?}"),
        };
        let mut layers = LayerTree::new();
        let default = add(&mut layers, "D", bc(140, -50, false));
        let legacy = add(&mut layers, "L", bc(90, -90, true));
        let range_of =
            |ws: &crate::Workspace, s: S| match ws.tree.payload(controls.sliders[s.index()]) {
                Some(WidgetKind::Slider(state)) => (state.min, state.max, state.value),
                other => unreachable!("{other:?}"),
            };
        let _ = show(&mut ws, controls, &layers, Some(default));
        assert_eq!(range_of(&ws, S::Brightness), (-150.0, 150.0, 140.0));
        assert_eq!(range_of(&ws, S::Contrast), (-50.0, 100.0, -50.0));
        let _ = show(&mut ws, controls, &layers, Some(legacy));
        assert_eq!(range_of(&ws, S::Brightness), (-100.0, 100.0, 90.0));
        assert_eq!(range_of(&ws, S::Contrast), (-100.0, 100.0, -90.0));
        // Back to the default mode: the range narrows again.
        let _ = show(&mut ws, controls, &layers, Some(default));
        assert_eq!(range_of(&ws, S::Contrast).0, -50.0);
        // Toggling Use Legacy on the document narrows brightness to +-100
        // (the value was clamped by the edit, so the slider follows).
        let toggled = match layers
            .adjustment(default)
            .and_then(Adjustment::brightness_contrast)
        {
            Some(p) => Adjustment::BrightnessContrast(p.with_legacy(true)),
            None => unreachable!("a Brightness/Contrast layer"),
        };
        let _ = layers.set_adjustment(default, toggled);
        let _ = show(&mut ws, controls, &layers, Some(default));
        assert_eq!(range_of(&ws, S::Brightness), (-100.0, 100.0, 100.0));
        assert_eq!(range_of(&ws, S::Contrast), (-100.0, 100.0, -50.0));
    }

    /// 0.245.0: Channel Mixer and Selective Color. The sliders read and write
    /// the row or range of the selector's tab; the Gray mix is the first row
    /// whatever tab is selected; the two-step Selective Color selector maps to
    /// the nine ranges; the readouts name the row or range; the selectors
    /// are live only for their kind.
    #[test]
    fn channel_mixer_and_selective_color_editors_follow_their_selectors() {
        let mut ws = crate::build_workspace(&scales());
        let controls = built(&mut ws);
        let at = |ws: &crate::Workspace, s: S| slider_state(&ws.tree, controls.sliders[s.index()]);
        let mixer = Adjustment::ChannelMixer(
            match aurora_core::ChannelMixerParams::new(
                [
                    [1.0, 2.0, 3.0, 4.0],
                    [5.0, 6.0, 7.0, 8.0],
                    [9.0, 10.0, 11.0, 12.0],
                ],
                false,
            ) {
                Ok(p) => p,
                Err(err) => unreachable!("{err:?}"),
            },
        );
        for (tab, row) in [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [9.0, 10.0, 11.0, 12.0],
        ]
        .into_iter()
        .enumerate()
        {
            for (slot, s) in [S::MixerRed, S::MixerGreen, S::MixerBlue, S::MixerConstant]
                .into_iter()
                .enumerate()
            {
                assert_eq!(
                    s.value_of(&mixer, tab),
                    Some(f64::from(row[slot])),
                    "{tab} {s:?}"
                );
                let edited = s.with_value(&mixer, tab, 50.2);
                let want = edited
                    .as_ref()
                    .and_then(Adjustment::channel_mixer)
                    .map(|p| p.rows());
                let mut expected = mixer
                    .channel_mixer()
                    .map(|p| p.rows())
                    .unwrap_or_else(|| unreachable!("mixer"));
                expected[tab][slot] = 50.0;
                assert_eq!(want, Some(expected), "{tab} {s:?}");
            }
        }
        assert_eq!(S::MixerRed.value_of(&mixer, 3), None, "no such output");
        assert_eq!(S::MixerRed.with_value(&mixer, 3, 5.0), None);
        // The ranges: sources to 200, the constant to 100, both signed.
        assert_eq!(S::MixerBlue.range(), (-200.0, 200.0));
        assert_eq!(S::MixerConstant.range(), (-100.0, 100.0));
        assert_eq!(S::MixerRed.snap(250.0), 200.0);
        assert_eq!(S::MixerConstant.snap(-250.0), -100.0);
        // Monochrome: the first row whatever the tab.
        let mono = Adjustment::ChannelMixer(
            mixer
                .channel_mixer()
                .map(|p| p.with_monochrome(true))
                .unwrap_or_else(|| unreachable!("mixer")),
        );
        assert_eq!(S::MixerGreen.value_of(&mono, 2), Some(2.0));
        let edited = S::MixerGreen.with_value(&mono, 2, 77.0);
        assert_eq!(
            edited
                .as_ref()
                .and_then(Adjustment::channel_mixer)
                .map(|p| p.rows()[0][1]),
            Some(77.0)
        );
        // In the tree: the output selector is live only for the mixer and idle in Monochrome.
        let mut layers = LayerTree::new();
        let id = add(&mut layers, "mix", mixer.clone());
        let _ = show(&mut ws, controls, &layers, Some(id));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::ChannelMixer)
        );
        assert!(
            widgets::tab_bar_state(&ws.tree, controls.mixer_output).is_ok_and(|t| !t.is_disabled())
        );
        assert!(
            widgets::tab_bar_state(&ws.tree, controls.selective_range)
                .is_ok_and(|t| t.is_disabled())
        );
        assert!(
            widgets::tab_bar_state(&ws.tree, controls.balance_range).is_ok_and(|t| t.is_disabled())
        );
        assert!(!super::adjust_mixer_mono_checked(&ws.tree, controls));
        assert_eq!(readout(&ws, controls, S::MixerRed), "Red output, Red: +1");
        let _ = widgets::select_tab(&mut ws.tree, controls.mixer_output, 2);
        let _ = show(&mut ws, controls, &layers, Some(id));
        assert_eq!(
            super::adjust_tab(&ws.tree, controls, AdjustKind::ChannelMixer),
            2
        );
        assert_eq!(at(&ws, S::MixerBlue), (11.0, false));
        assert_eq!(
            readout(&ws, controls, S::MixerBlue),
            "Blue output, Blue: +11"
        );
        assert_eq!(
            super::adjust_tab(&ws.tree, controls, AdjustKind::Threshold),
            0
        );
        let _ = layers.set_adjustment(id, mono);
        let _ = show(&mut ws, controls, &layers, Some(id));
        assert!(super::adjust_mixer_mono_checked(&ws.tree, controls));
        assert!(
            widgets::tab_bar_state(&ws.tree, controls.mixer_output).is_ok_and(|t| t.is_disabled())
        );
        assert_eq!(at(&ws, S::MixerBlue), (3.0, false), "the Gray row");
        assert_eq!(readout(&ws, controls, S::MixerBlue), "Gray, Blue: +3");

        // Selective Color: nine ranges in two steps.
        let mut ranges = [[0.0_f32; 4]; 9];
        for (i, row) in ranges.iter_mut().enumerate() {
            *row = [i as f32, -(i as f32), 10.0 + i as f32, -10.0 - i as f32];
        }
        let selective = Adjustment::SelectiveColor(
            match aurora_core::SelectiveColorParams::new(ranges, true) {
                Ok(p) => p,
                Err(err) => unreachable!("{err:?}"),
            },
        );
        for (range, row) in ranges.iter().enumerate() {
            for (slot, s) in [
                S::SelectiveCyan,
                S::SelectiveMagenta,
                S::SelectiveYellow,
                S::SelectiveBlack,
            ]
            .into_iter()
            .enumerate()
            {
                assert_eq!(s.value_of(&selective, range), Some(f64::from(row[slot])));
                let edited = s.with_value(&selective, range, -5.0);
                let mut want = ranges;
                want[range][slot] = -5.0;
                assert_eq!(
                    edited
                        .as_ref()
                        .and_then(Adjustment::selective_color)
                        .map(|p| p.ranges()),
                    Some(want)
                );
            }
        }
        assert_eq!(S::SelectiveCyan.value_of(&selective, 9), None);
        assert_eq!(S::SelectiveBlack.range(), (-100.0, 100.0));
        let id = add(&mut layers, "sel", selective);
        let _ = show(&mut ws, controls, &layers, Some(id));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::SelectiveColor)
        );
        assert!(super::adjust_selective_absolute_checked(&ws.tree, controls));
        assert!(
            widgets::tab_bar_state(&ws.tree, controls.mixer_output).is_ok_and(|t| t.is_disabled())
        );
        assert!(
            widgets::tab_bar_state(&ws.tree, controls.selective_range)
                .is_ok_and(|t| !t.is_disabled())
        );
        // Group 0 + colour bar tab 3 is Cyans; groups 1..3 are Whites, Neutrals, Blacks.
        let _ = widgets::select_tab(&mut ws.tree, controls.selective_range, 3);
        for (group, colour, want) in [(0, 3, 3), (0, 5, 5), (1, 0, 6), (2, 4, 7), (3, 1, 8)] {
            let _ = widgets::select_tab(&mut ws.tree, controls.selective_group, group);
            let _ = widgets::select_tab(&mut ws.tree, controls.selective_range, colour);
            let _ = show(&mut ws, controls, &layers, Some(id));
            let tab = super::adjust_tab(&ws.tree, controls, AdjustKind::SelectiveColor);
            assert_eq!(tab, want, "group {group} colour {colour}");
            assert_eq!(at(&ws, S::SelectiveCyan).0, want as f64);
            assert_eq!(
                widgets::tab_bar_state(&ws.tree, controls.selective_range)
                    .is_ok_and(|t| t.is_disabled()),
                group != 0,
                "the colour bar is idle outside group 0"
            );
        }
        assert_eq!(
            readout(&ws, controls, S::SelectiveBlack),
            "Blacks, Black: -18"
        );
        // Hidden again for another layer.
        let _ = show(&mut ws, controls, &layers, None);
        assert_eq!(adjust_controls_shown(&ws.tree, controls), None);
    }

    // ---- 0.248.0: the Gradient Map editor ----

    fn gradient(stops: &[(f32, [f32; 3])], reverse: bool) -> Adjustment {
        let stops = stops
            .iter()
            .map(|&(p, c)| aurora_core::GradientStop::new(p, c))
            .collect();
        match aurora_core::GradientMapParams::new(stops, reverse, false) {
            Ok(p) => Adjustment::GradientMap(p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn label_text(ws: &crate::Workspace, id: aurora_widgets::WidgetId) -> String {
        match ws.tree.payload(id) {
            Some(WidgetKind::Label(label)) => label.text.clone(),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn a_two_stop_gradient_edits_its_start_and_end_colours() {
        let scales = scales();
        let mut ws = crate::build_workspace(&scales);
        let controls = match insert_adjust_controls(&mut ws.tree, ws.properties.root, &scales) {
            Ok(c) => c,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut layers = LayerTree::new();
        let id = add(
            &mut layers,
            "G",
            gradient(&[(0.0, [1.0, 0.0, 0.0]), (1.0, [0.0, 0.2, 1.0])], true),
        );
        let _ = show(&mut ws, controls, &layers, Some(id));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::GradientMap)
        );
        let at = |ws: &crate::Workspace, s: S| slider_state(&ws.tree, controls.sliders[s.index()]);
        assert_eq!(at(&ws, S::GradientStartRed), (255.0, false));
        assert_eq!(at(&ws, S::GradientStartGreen), (0.0, false));
        assert_eq!(at(&ws, S::GradientEndGreen), (51.0, false));
        assert_eq!(at(&ws, S::GradientEndBlue), (255.0, false));
        assert!(super::adjust_gradient_reverse_checked(&ws.tree, controls));
        assert_eq!(label_text(&ws, controls.gradient_summary), "2 colour stops");
        assert_eq!(
            label_text(&ws, controls.readouts[S::GradientEndRed.index()]),
            "End red: 0"
        );
        // An edit changes only its channel of its stop and keeps the rest.
        let a = layers.adjustment(id).cloned().unwrap_or(Adjustment::Invert);
        let edited = S::GradientStartGreen
            .with_value(&a, 0, 128.0)
            .and_then(|e| e.gradient_map().cloned());
        let Some(edited) = edited else {
            unreachable!("two-stop edit")
        };
        assert_eq!(edited.stops()[0].colour, [1.0, 128.0 / 255.0, 0.0]);
        assert_eq!(edited.stops()[1].colour, [0.0, 0.2, 1.0]);
        assert!(edited.reverse());
        // The other sliders of other kinds give nothing for this layer.
        assert!(S::Red.with_value(&a, 0, 5.0).is_none());
    }

    #[test]
    fn a_gradient_with_more_stops_shows_a_summary_and_no_editable_colours() {
        let scales = scales();
        let mut ws = crate::build_workspace(&scales);
        let controls = match insert_adjust_controls(&mut ws.tree, ws.properties.root, &scales) {
            Ok(c) => c,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut layers = LayerTree::new();
        let three = gradient(
            &[(0.0, [0.0; 3]), (0.5, [1.0, 0.0, 0.0]), (1.0, [1.0; 3])],
            false,
        );
        let id = add(&mut layers, "G3", three.clone());
        let _ = show(&mut ws, controls, &layers, Some(id));
        assert_eq!(
            adjust_controls_shown(&ws.tree, controls),
            Some(AdjustKind::GradientMap)
        );
        assert!(label_text(&ws, controls.gradient_summary).starts_with("3 colour stops"));
        for s in [
            S::GradientStartRed,
            S::GradientStartGreen,
            S::GradientStartBlue,
            S::GradientEndRed,
            S::GradientEndGreen,
            S::GradientEndBlue,
        ] {
            assert!(
                slider_state(&ws.tree, controls.sliders[s.index()]).1,
                "{s:?} is disabled for a three-stop gradient"
            );
            assert!(s.value_of(&three, 0).is_none());
            assert!(s.with_value(&three, 0, 10.0).is_none());
        }
        assert!(!super::adjust_gradient_reverse_checked(&ws.tree, controls));
        // Reverse stays live, and back on a two-stop layer the sliders wake.
        let two = add(
            &mut layers,
            "G2",
            Adjustment::GradientMap(aurora_core::GradientMapParams::black_to_white()),
        );
        let _ = show(&mut ws, controls, &layers, Some(two));
        assert!(!slider_state(&ws.tree, controls.sliders[S::GradientEndRed.index()]).1);
        assert_eq!(
            slider_state(&ws.tree, controls.sliders[S::GradientEndRed.index()]).0,
            255.0
        );
        // Hidden for another kind.
        let _ = show(&mut ws, controls, &layers, None);
        assert_eq!(adjust_controls_shown(&ws.tree, controls), None);
        assert!(slider_state(&ws.tree, controls.sliders[S::GradientEndRed.index()]).1);
        assert_eq!(super::gradient_summary_text(2), "2 colour stops");
        assert!(super::gradient_summary_text(7).starts_with("7 colour stops"));
    }
}
