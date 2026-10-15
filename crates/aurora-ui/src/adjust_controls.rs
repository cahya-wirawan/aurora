//! The Properties panel's **Vibrance, Exposure, Photo Filter, Black & White
//! and Color Balance editors** (0.239.0, the last two 0.240.0): one hidden
//! column holding five groups of sliders with a readout each (and the
//! checkboxes and the Color Balance range selector), of which at most one
//! is shown, the one matching the active layer.
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
    BrightnessContrastParams, COLOR_BALANCE_MAX, COLOR_BALANCE_RANGES, CONTRAST_MAX,
    DEFAULT_TINT_HUE, DEFAULT_TINT_SATURATION, EXPOSURE_MAX, ExposureParams, GAMMA_MAX, GAMMA_MIN,
    LEGACY_MAX, OFFSET_MAX, POSTERIZE_DEFAULT, POSTERIZE_MAX, POSTERIZE_MIN, PhotoFilterParams,
    THRESHOLD_DEFAULT, THRESHOLD_MAX, THRESHOLD_MIN, VibranceParams,
};
use aurora_doc::{Adjustment, LayerId, LayerTree};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{Display, FlexDirection, Size, Style};

/// How many sliders the editors hold.
pub const ADJUST_SLIDER_COUNT: usize = 24;
/// How many editor groups there are.
pub const ADJUST_KIND_COUNT: usize = 9;

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
            _ => None,
        }
    }
}

/// One of the twenty-four sliders.
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
    /// Brightness/Contrast's brightness, `-150..=150` (`-100..=100` with
    /// Use Legacy: an edit is clamped to the mode's range).
    Brightness,
    /// Its contrast, `-100..=100` (`-50..=100` in the default mode).
    Contrast,
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
            Self::Red => "Red",
            Self::Green => "Green",
            Self::Blue => "Blue",
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
            Self::Red | Self::Green | Self::Blue => (0.0, 255.0),
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
        {
            format!("{}: {value:+.places$}", self.short_label())
        } else {
            format!("{}: {value:.places$}", self.short_label())
        }
    }

    /// The slider position for `adjustment`, `None` when it is not this
    /// slider's adjustment. `range` is the Color Balance range selector's
    /// tab (`0` shadows, `1` midtones, `2` highlights); every other slider
    /// ignores it.
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
    #[allow(clippy::cast_possible_truncation)]
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
        let invert_note = widgets::insert_label(
            tree,
            group_of(AdjustKind::Invert)?,
            scales,
            "Invert has no settings.".to_owned(),
        )?;
        for slider in AdjustSlider::ALL {
            let group = group_of(slider.kind())?;
            // Controls that sit before a slider in its group.
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
            Ok(sliders),
            Ok(readouts),
            Ok(groups),
        ) = (
            preserve,
            tint,
            balance_preserve,
            balance_range,
            legacy,
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
        };
        set_shown(tree, controls, None, false)?;
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
fn set_shown(
    tree: &mut WidgetTree<WidgetKind>,
    controls: AdjustControls,
    kind: Option<AdjustKind>,
    tint_on: bool,
) -> Result<bool, WidgetError> {
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
        let live = kind == Some(slider.kind()) && (!slider.is_tint() || tint_on);
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
    let tabs_live = kind == Some(AdjustKind::ColorBalance);
    if widgets::tab_bar_state(tree, controls.balance_range)?.is_disabled() == tabs_live {
        widgets::set_tab_bar_disabled(tree, controls.balance_range, !tabs_live)?;
        changed = true;
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
        return set_shown(tree, controls, None, false);
    };
    let kind = AdjustKind::of(adjustment);
    let tint_on = adjustment.black_white().is_some_and(|p| p.tint());
    let mut changed = set_shown(tree, controls, kind, tint_on)?;
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
    let range = adjust_balance_range(tree, controls);
    for slider in AdjustSlider::ALL
        .into_iter()
        .filter(|s| Some(s.kind()) == kind)
    {
        let (Some(&id), Some(&readout)) = (
            controls.sliders.get(slider.index()),
            controls.readouts.get(slider.index()),
        ) else {
            continue;
        };
        let Some(value) = slider.value_of(adjustment, range) else {
            continue;
        };
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
        changed |= widgets::set_label_text(tree, readout, &slider.readout(shown_value))?;
    }
    Ok(changed)
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
#[allow(clippy::float_cmp, clippy::indexing_slicing)]
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
        for s in S::ALL {
            let base = match s.kind() {
                AdjustKind::Vibrance => &v,
                AdjustKind::Exposure => &e,
                AdjustKind::PhotoFilter => &f,
                AdjustKind::BlackWhite => &bw,
                AdjustKind::ColorBalance => &cb,
            };
            let (min, max) = s.range();
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
}
