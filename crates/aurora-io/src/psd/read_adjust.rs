//! Vibrance, Exposure, Photo Filter, Black & White and Color Balance
//! adjustment layers on the PSD reader (0.241.0): the tagged blocks
//! `vibA`, `expA`, `phfl`, `blwh` and `blnc`.
//!
//! Every layout below was checked against **real Photoshop output**, the
//! psd-tools fixtures (`layers/{vibrance,exposure,photo-filter,
//! black-and-white,color-balance}.psd`, `fill_adjustments.psd`,
//! `adjustments/exposure_rgb.psd`, `clipping-mask2.psd`) and against
//! psd-tools 1.17.4's parsers, not written from memory:
//!
//! * `vibA` and `blwh`: a descriptor block. A `u32` descriptor version
//!   (`16`, *without* the object-version `u32` that the effects blocks
//!   carry), then an action descriptor ([`super::descriptor`]). Vibrance
//!   holds `vibrance` and `Strt` (Saturation), both `long`, `-100..=100`;
//!   Photoshop omits a key at its default `0` (`layers/vibrance.psd` has an
//!   empty descriptor). Black & White holds `Rd  `, `Yllw`, `Grn `, `Cyn `,
//!   `Bl  `, `Mgnt` (`long`, `-200..=300`, defaults `40, 60, 40, 60, 20,
//!   80`), `useTint` (`bool`), `tintColor` (an `RGBC` object of three
//!   `doub` 0..255: `Rd  `, `Grn `, `Bl  `; default `225, 211, 179`),
//!   `bwPresetKind` and `blackAndWhitePresetFileName`.
//! * `expA`: `u16` version `1`, then `f32` exposure, offset and gamma
//!   (`layers/exposure.psd` is `0, 0, 1`; `exposure_rgb.psd` holds
//!   `2.03, 0.0775, 1.52` and three more). The gamma is the Gamma
//!   Correction slider's value (neutral `1.0`).
//! * `phfl`: `u16` version. Version 2 (every fixture): `u16` colour space,
//!   four `u16` components, `u32` density (percent), `u8` preserve
//!   luminosity. Colour space `7` is Lab, components `(L * 100, a * 100, b *
//!   100)` ([`super::lab`]). Version 3 (three `u32` XYZ values) and any
//!   other colour space have no fixture and are left out and reported
//!   rather than guessed.
//! * `blnc`: nine `i16` (shadows, midtones, highlights, each cyan-red,
//!   magenta-green, yellow-blue, `-100..=100`), then a `u8` preserve
//!   luminosity and a pad byte. There is no version field.
//!
//! **Out-of-range values are left out, never repaired**: a layer whose
//! value falls outside Aurora's validated range (`aurora_core`'s param
//! types) is dropped with its own report line, exactly like an unreadable
//! `hue2`. The only values changed are documented where they occur: the
//! Photo Filter's Lab colour is clipped to the sRGB gamut, and the Black &
//! White tint colour is reduced to the HSL hue and saturation Aurora's tint
//! uses ([`tint_hue_saturation`]).
//!
//! * 0.244.0, also checked against `adjustments/{invert,threshold,
//!   posterize,brightnesscontrast}*_rgb.psd`: `nvrt` is an empty block;
//!   `thrs` and `post` are a `u16` level and two zero bytes (`00e80000` is
//!   level 232); `brit` is `i16` brightness, `i16` contrast, `i16` mean, a
//!   `u8` Lab-only flag and a pad byte (the Use Legacy layer holds its
//!   values here: `0014 0039 007f 0000`); a default-mode layer writes eight
//!   zero bytes to `brit` and keeps its values in a `CgEd` descriptor
//!   (`Vrsn` 1, `Brgh`, `Cntr`, `means`, `Lab `, `useLegacy`, `Auto`).
//!   Layers using Auto, Lab-only or a mean other than 127 are left out.
//!
//! * 0.245.0 (revised): a Monochrome `mixr` and a `selc` that adjusts anything
//!   are **left out** with their own report line, because psd-tools'
//!   parsers (`psd/adjustments.py`, `ChannelMixer` and `SelectiveColor`)
//!   name neither the Gray row nor the order of the ten plates and no
//!   fixture constrains them. Neutral layers (any method) and non-Monochrome
//!   mixers import.
//! * 0.245.0: `mixr` and `selc`, checked on `layers/channel-mixer.psd`,
//!   `layers/selective-color.psd` and `fill_adjustments.psd` (the neutral
//!   layers; 44 and 84 bytes) and against psd-tools 1.17.4's parsers. **Only
//!   the neutral case is verified**: the Monochrome row of `mixr` and the
//!   plate order of `selc` are not (no fixture has either), and the formulas
//!   applied to a non-neutral layer are unvalidated approximations (see
//!   `aurora-filters`' `channel_mixer` and `selective_color`).
//!
//! Every parser is total: short data, an unknown version or a hostile
//! descriptor is an `Err`, never a panic, and nothing allocates more than
//! the descriptor parser's bounded, byte-checked amount.

use super::descriptor::{self, Descriptor, ItemBudget};
use super::{Note, Reader};
use aurora_doc::Adjustment;

/// The longest descriptor block read: a real one is under 400 bytes.
const MAX_DESCRIPTOR_BYTES: usize = 64 * 1024;

/// How many descriptor values one block may hold: a real Black & White
/// block holds fourteen.
const MAX_DESCRIPTOR_ITEMS: u64 = 256;

/// Which of the five kinds a report line is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Kind {
    Vibrance,
    Exposure,
    PhotoFilter,
    BlackWhite,
    ColorBalance,
    Invert,
    Threshold,
    Posterize,
    BrightnessContrast,
    ChannelMixer,
    SelectiveColor,
    GradientMap,
}

impl Kind {
    /// The name Photoshop and Aurora give the adjustment.
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Vibrance => "Vibrance",
            Self::Exposure => "Exposure",
            Self::PhotoFilter => "Photo Filter",
            Self::BlackWhite => "Black & White",
            Self::ColorBalance => "Color Balance",
            Self::Invert => "Invert",
            Self::Threshold => "Threshold",
            Self::Posterize => "Posterize",
            Self::BrightnessContrast => "Brightness/Contrast",
            Self::ChannelMixer => "Channel Mixer",
            Self::SelectiveColor => "Selective Color",
            Self::GradientMap => "Gradient Map",
        }
    }
}

/// A parsed `vibA` block: the two sliders, `0` where Photoshop left the
/// key out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct VibranceBlock {
    pub vibrance: f64,
    pub saturation: f64,
}

/// A parsed `expA` block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ExposureBlock {
    pub exposure: f32,
    pub offset: f32,
    pub gamma: f32,
}

/// A Photo Filter colour as the file holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PhotoColour {
    /// Colour space 7: `(L, a, b)` already divided by 100.
    Lab(f64, f64, f64),
    /// Version 3, or a colour space other than Lab: unverified.
    Unsupported,
}

/// A parsed `phfl` block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PhotoFilterBlock {
    pub colour: PhotoColour,
    pub density: u32,
    pub preserve_luminosity: bool,
}

/// A parsed `blwh` block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BlackWhiteBlock {
    /// Reds, yellows, greens, cyans, blues, magentas.
    pub weights: [f64; 6],
    pub tint: bool,
    /// The tint colour, `0..=255` per channel.
    pub tint_rgb: [f64; 3],
}

/// A parsed `blnc` block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ColorBalanceBlock {
    /// Shadows, midtones, highlights; cyan-red, magenta-green, yellow-blue.
    pub ranges: [[i16; 3]; 3],
    pub preserve_luminosity: bool,
}

/// A parsed legacy-layout `brit` block (0.244.0): `i16` brightness, `i16`
/// contrast, `i16` mean, a `u8` Lab-only flag and a pad byte, 8 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BritBlock {
    pub brightness: i16,
    pub contrast: i16,
    pub mean: i16,
    pub lab: bool,
}

/// A parsed `mixr` block (0.245.0): version `1`, the Monochrome flag, then
/// four output rows of five `i16` (red, green, blue and a fourth source that
/// only CMYK documents use, then the constant). Only the first three rows
/// matter for RGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MixerBlock {
    pub monochrome: bool,
    /// `[output][red, green, blue, constant]` for the red, green and blue
    /// outputs.
    pub rows: [[i16; 4]; 3],
}

/// A parsed `selc` block (0.245.0): version `1`, the method (`0` Relative,
/// `1` Absolute) and ten plates of four `i16` (cyan, magenta, yellow,
/// black). The first plate is unused; the other nine are Reds, Yellows,
/// Greens, Cyans, Blues, Magentas, Whites, Neutrals, Blacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SelectiveBlock {
    pub absolute: bool,
    pub unused: [i16; 4],
    pub ranges: [[i16; 4]; 9],
}

/// The `CgEd` descriptor of a non-legacy Brightness/Contrast layer
/// (0.244.0): `Vrsn`, `Brgh`, `Cntr`, `means`, `Lab `, `useLegacy`, `Auto`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CgedBlock {
    pub version: f64,
    pub brightness: f64,
    pub contrast: f64,
    pub means: f64,
    pub lab: bool,
    pub legacy: bool,
    pub auto: bool,
}

/// The last block of one of the kinds a record carried.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Parsed {
    /// `nvrt`: an empty block.
    Invert(Result<(), ()>),
    /// `thrs`: the level.
    Threshold(Result<u16, ()>),
    /// `post`: the level count.
    Posterize(Result<u16, ()>),
    /// `brit` and the record's `CgEd`, when it had one.
    Brightness(Result<BritBlock, ()>, Option<Result<CgedBlock, ()>>),
    Vibrance(Result<VibranceBlock, ()>),
    Exposure(Result<ExposureBlock, ()>),
    PhotoFilter(Result<PhotoFilterBlock, ()>),
    BlackWhite(Result<BlackWhiteBlock, ()>),
    ColorBalance(Result<ColorBalanceBlock, ()>),
    /// `mixr` (0.245.0).
    ChannelMixer(Result<MixerBlock, ()>),
    /// `selc` (0.245.0).
    SelectiveColor(Result<SelectiveBlock, ()>),
    /// `grdm` (0.248.0).
    GradientMap(Result<GradientBlock, ()>),
    /// `clrL` (0.248.0): never applied, read only to say why.
    ColorLookup(Result<LookupBlock, ()>),
}

/// The parser for `key`, or `None` for a key that is not one of the five.
pub(super) fn parse_block(key: [u8; 4], data: &[u8]) -> Option<Parsed> {
    Some(match &key {
        b"vibA" => Parsed::Vibrance(parse_vibrance(data)),
        b"expA" => Parsed::Exposure(parse_exposure(data)),
        b"phfl" => Parsed::PhotoFilter(parse_photo_filter(data)),
        b"blwh" => Parsed::BlackWhite(parse_black_white(data)),
        b"blnc" => Parsed::ColorBalance(parse_color_balance(data)),
        b"nvrt" => Parsed::Invert(if data.is_empty() { Ok(()) } else { Err(()) }),
        b"thrs" => Parsed::Threshold(parse_level(data)),
        b"post" => Parsed::Posterize(parse_level(data)),
        b"brit" => Parsed::Brightness(parse_brit(data), None),
        b"mixr" => Parsed::ChannelMixer(parse_mixer(data)),
        b"selc" => Parsed::SelectiveColor(parse_selective(data)),
        b"grdm" => Parsed::GradientMap(parse_gradient_map(data)),
        b"clrL" => Parsed::ColorLookup(parse_color_lookup(data)),
        _ => return None,
    })
}

/// Whether `key` is one of the five blocks.
pub(super) fn is_key(key: [u8; 4]) -> bool {
    matches!(
        &key,
        b"vibA"
            | b"expA"
            | b"phfl"
            | b"blwh"
            | b"blnc"
            | b"nvrt"
            | b"thrs"
            | b"post"
            | b"brit"
            | b"mixr"
            | b"selc"
            | b"grdm"
            | b"clrL"
    )
}

/// Adds a record's `CgEd` block to a parsed `brit` (no effect on any other
/// kind). Returns whether it attached.
pub(super) fn attach_cged(parsed: &mut Parsed, data: &[u8]) -> bool {
    if let Parsed::Brightness(_, slot) = parsed {
        *slot = Some(parse_cged(data));
        return true;
    }
    false
}

// ---------------------------------------------------------------------
// Descriptor helpers
// ---------------------------------------------------------------------

/// The descriptor of a `vibA`/`blwh` block.
fn descriptor_of(data: &[u8]) -> Result<Descriptor, ()> {
    if data.len() > MAX_DESCRIPTOR_BYTES {
        return Err(());
    }
    let mut budget = ItemBudget {
        remaining: MAX_DESCRIPTOR_ITEMS,
    };
    descriptor::parse_adjustment_block(data, &mut budget).map_err(|_| ())
}

/// A numeric item, `default` when the key is absent; a key that is not a
/// finite number is unreadable.
fn number(d: &Descriptor, key: &[u8], default: f64) -> Result<f64, ()> {
    match d.get(key) {
        None => Ok(default),
        Some(v) => v.number().filter(|n| n.is_finite()).ok_or(()),
    }
}

// ---------------------------------------------------------------------
// Vibrance
// ---------------------------------------------------------------------

fn parse_vibrance(data: &[u8]) -> Result<VibranceBlock, ()> {
    let d = descriptor_of(data)?;
    Ok(VibranceBlock {
        vibrance: number(&d, b"vibrance", 0.0)?,
        saturation: number(&d, b"Strt", 0.0)?,
    })
}

/// A parsed block as Aurora's Vibrance. A Grayscale file's layers are not
/// imported (as for Curves: nothing verified how Photoshop applies them).
pub(super) fn vibrance_adjustment(block: &VibranceBlock, gray: bool) -> Result<Adjustment, Note> {
    if gray {
        return Err(Note::Grayscale(Kind::Vibrance));
    }
    #[allow(clippy::cast_possible_truncation)] // validated to -100..=100 next
    let params = aurora_core::VibranceParams::new(block.vibrance as f32, block.saturation as f32)
        .map_err(|_| Note::Unreadable(Kind::Vibrance))?;
    Ok(Adjustment::Vibrance(params))
}

// ---------------------------------------------------------------------
// Exposure
// ---------------------------------------------------------------------

fn parse_exposure(data: &[u8]) -> Result<ExposureBlock, ()> {
    let mut r = Reader::new(data);
    let unreadable = |_| ();
    if r.u16("exposure version").map_err(unreadable)? != 1 {
        return Err(());
    }
    let mut float = || {
        r.u32("exposure value")
            .map(f32::from_bits)
            .map_err(unreadable)
    };
    Ok(ExposureBlock {
        exposure: float()?,
        offset: float()?,
        gamma: float()?,
    })
}

/// A parsed block as Aurora's Exposure: exposure `-20..=20`, offset
/// `-0.5..=0.5`, gamma `0.10..=9.99`. Photoshop's Gamma Correction goes
/// down to `0.01`; a layer below Aurora's `0.10` is left out and reported,
/// not clamped.
pub(super) fn exposure_adjustment(block: &ExposureBlock, gray: bool) -> Result<Adjustment, Note> {
    if gray {
        return Err(Note::Grayscale(Kind::Exposure));
    }
    aurora_core::ExposureParams::new(block.exposure, block.offset, block.gamma)
        .map(Adjustment::Exposure)
        .map_err(|_| Note::Unreadable(Kind::Exposure))
}

// ---------------------------------------------------------------------
// Photo Filter
// ---------------------------------------------------------------------

/// The colour space code of Lab in a `phfl` block.
const COLOUR_SPACE_LAB: u16 = 7;

fn parse_photo_filter(data: &[u8]) -> Result<PhotoFilterBlock, ()> {
    let mut r = Reader::new(data);
    let unreadable = |_| ();
    let version = r.u16("photo filter version").map_err(unreadable)?;
    let colour = match version {
        2 => {
            let space = r.u16("photo filter colour space").map_err(unreadable)?;
            let l = r.u16("photo filter L").map_err(unreadable)?;
            let a = r.i16("photo filter a").map_err(unreadable)?;
            let b = r.i16("photo filter b").map_err(unreadable)?;
            r.u16("photo filter component").map_err(unreadable)?;
            if space == COLOUR_SPACE_LAB {
                PhotoColour::Lab(
                    f64::from(l) / 100.0,
                    f64::from(a) / 100.0,
                    f64::from(b) / 100.0,
                )
            } else {
                PhotoColour::Unsupported
            }
        }
        // Three `u32` XYZ values: no fixture to check their scale against.
        3 => {
            r.skip(12, "photo filter xyz").map_err(unreadable)?;
            PhotoColour::Unsupported
        }
        _ => return Err(()),
    };
    let density = r.u32("photo filter density").map_err(unreadable)?;
    let preserve_luminosity = r.u8("photo filter luminosity").map_err(unreadable)? != 0;
    Ok(PhotoFilterBlock {
        colour,
        density,
        preserve_luminosity,
    })
}

/// A parsed block as Aurora's Photo Filter. The Lab colour is converted
/// to sRGB with D50 white ([`super::lab`]) and clipped to the gamut; a
/// density outside `1..=100` is unreadable.
pub(super) fn photo_filter_adjustment(
    block: &PhotoFilterBlock,
    gray: bool,
) -> Result<Adjustment, Note> {
    if gray {
        return Err(Note::Grayscale(Kind::PhotoFilter));
    }
    let PhotoColour::Lab(l, a, b) = block.colour else {
        return Err(Note::PhotoFilterColour);
    };
    #[allow(clippy::cast_precision_loss)] // a percent
    let density = block.density.min(1_000) as f32;
    aurora_core::PhotoFilterParams::new(
        super::lab::lab_to_srgb(l, a, b),
        density,
        block.preserve_luminosity,
    )
    .map(Adjustment::PhotoFilter)
    .map_err(|_| Note::Unreadable(Kind::PhotoFilter))
}

// ---------------------------------------------------------------------
// Black & White
// ---------------------------------------------------------------------

/// The key of each of the six sliders, reds first.
const BW_KEYS: [&[u8]; 6] = [b"Rd  ", b"Yllw", b"Grn ", b"Cyn ", b"Bl  ", b"Mgnt"];

/// Photoshop's default tint colour.
const BW_DEFAULT_TINT: [f64; 3] = [225.0, 211.0, 179.0];

fn parse_black_white(data: &[u8]) -> Result<BlackWhiteBlock, ()> {
    let d = descriptor_of(data)?;
    let mut weights = [0.0_f64; 6];
    for ((slot, key), default) in weights
        .iter_mut()
        .zip(BW_KEYS)
        .zip(aurora_core::BLACK_WHITE_DEFAULTS)
    {
        *slot = number(&d, key, f64::from(default))?;
    }
    let tint = match d.get(b"useTint") {
        None => false,
        Some(v) => v.bool().ok_or(())?,
    };
    let tint_rgb = match d.get(b"tintColor") {
        None => BW_DEFAULT_TINT,
        Some(v) => {
            let colour = v.descriptor().ok_or(())?;
            [
                number(colour, b"Rd  ", BW_DEFAULT_TINT[0])?,
                number(colour, b"Grn ", BW_DEFAULT_TINT[1])?,
                number(colour, b"Bl  ", BW_DEFAULT_TINT[2])?,
            ]
        }
    };
    Ok(BlackWhiteBlock {
        weights,
        tint,
        tint_rgb,
    })
}

/// The HSL hue (degrees) and saturation (percent) of an `0..=255` RGB
/// colour: Aurora's tint is "this hue and saturation at the grey's
/// lightness", so the stored colour is reduced to the two numbers its
/// pixel math uses. A grey has no hue and keeps Aurora's default tint hue.
/// **A choice, not compared with Photoshop**: no corpus file has Tint on.
pub(super) fn tint_hue_saturation(rgb: [f64; 3]) -> (f32, f32) {
    let [r, g, b] = rgb.map(|c| c.clamp(0.0, 255.0) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let delta = max - min;
    let lightness = f64::midpoint(max, min);
    let denominator = 1.0 - (2.0 * lightness - 1.0).abs();
    if delta <= 0.0 || denominator <= 0.0 {
        return (aurora_core::DEFAULT_TINT_HUE, 0.0);
    }
    let hue = if (max - r).abs() < f64::EPSILON {
        ((g - b) / delta).rem_euclid(6.0)
    } else if (max - g).abs() < f64::EPSILON {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    } * 60.0;
    #[allow(clippy::cast_possible_truncation)] // 0..=360 and 0..=100
    (hue as f32, (delta / denominator * 100.0) as f32)
}

/// The `0..=255` RGB colour whose HSL hue and saturation are `hue` degrees
/// and `saturation` percent, at lightness 0.5 (where the HSL saturation is
/// the chroma): the inverse of [`tint_hue_saturation`], written as the
/// Black & White `tintColor`.
pub(super) fn tint_rgb(hue: f32, saturation: f32) -> [f64; 3] {
    let chroma = f64::from(saturation.clamp(0.0, 100.0)) / 100.0;
    let sector = f64::from(hue).rem_euclid(360.0) / 60.0;
    let second = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match sector {
        s if s < 1.0 => (chroma, second, 0.0),
        s if s < 2.0 => (second, chroma, 0.0),
        s if s < 3.0 => (0.0, chroma, second),
        s if s < 4.0 => (0.0, second, chroma),
        s if s < 5.0 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let offset = 0.5 - chroma / 2.0;
    [r, g, b].map(|c| (c + offset).clamp(0.0, 1.0) * 255.0)
}

/// A parsed block as Aurora's Black & White: sliders as stored (the same
/// `-200..=300` range), the tint colour as [`tint_hue_saturation`].
pub(super) fn black_white_adjustment(
    block: &BlackWhiteBlock,
    gray: bool,
) -> Result<Adjustment, Note> {
    if gray {
        return Err(Note::Grayscale(Kind::BlackWhite));
    }
    // A colour channel more than half a level outside 0..255 is damage.
    if block.tint_rgb.iter().any(|c| !(-0.5..=255.5).contains(c)) {
        return Err(Note::Unreadable(Kind::BlackWhite));
    }
    #[allow(clippy::cast_possible_truncation)] // validated by `new`
    let weights = block.weights.map(|w| w as f32);
    let (hue, saturation) = tint_hue_saturation(block.tint_rgb);
    aurora_core::BlackWhiteParams::new(weights, block.tint, hue, saturation)
        .map(Adjustment::BlackWhite)
        .map_err(|_| Note::Unreadable(Kind::BlackWhite))
}

// ---------------------------------------------------------------------
// Color Balance
// ---------------------------------------------------------------------

fn parse_color_balance(data: &[u8]) -> Result<ColorBalanceBlock, ()> {
    let mut r = Reader::new(data);
    let unreadable = |_| ();
    let mut ranges = [[0_i16; 3]; 3];
    for slot in ranges.iter_mut().flatten() {
        *slot = r.i16("color balance value").map_err(unreadable)?;
    }
    let preserve_luminosity = r.u8("color balance luminosity").map_err(unreadable)? != 0;
    Ok(ColorBalanceBlock {
        ranges,
        preserve_luminosity,
    })
}

/// A parsed block as Aurora's Color Balance (`-100..=100` sliders).
pub(super) fn color_balance_adjustment(
    block: &ColorBalanceBlock,
    gray: bool,
) -> Result<Adjustment, Note> {
    if gray {
        return Err(Note::Grayscale(Kind::ColorBalance));
    }
    aurora_core::ColorBalanceParams::new(
        block.ranges.map(|r| r.map(f32::from)),
        block.preserve_luminosity,
    )
    .map(Adjustment::ColorBalance)
    .map_err(|_| Note::Unreadable(Kind::ColorBalance))
}

// ---------------------------------------------------------------------
// Threshold, Posterize, Brightness/Contrast (0.244.0)
// ---------------------------------------------------------------------

/// A `thrs` or `post` block: a `u16` level and two zero bytes (checked on
/// the fixtures `adjustments/{threshold,posterize}_*.psd`, `00e80000` etc).
fn parse_level(data: &[u8]) -> Result<u16, ()> {
    let [hi, lo, pad_a, pad_b] = <[u8; 4]>::try_from(data).map_err(|_| ())?;
    if pad_a != 0 || pad_b != 0 {
        return Err(());
    }
    Ok(u16::from_be_bytes([hi, lo]))
}

fn parse_brit(data: &[u8]) -> Result<BritBlock, ()> {
    if data.len() != 8 {
        return Err(());
    }
    let mut r = Reader::new(data);
    let unreadable = |_| ();
    let brightness = r.i16("brightness").map_err(unreadable)?;
    let contrast = r.i16("contrast").map_err(unreadable)?;
    let mean = r.i16("mean").map_err(unreadable)?;
    let lab = r.u8("lab only").map_err(unreadable)? != 0;
    Ok(BritBlock {
        brightness,
        contrast,
        mean,
        lab,
    })
}

fn parse_cged(data: &[u8]) -> Result<CgedBlock, ()> {
    let d = descriptor_of(data)?;
    let flag = |key: &[u8]| match d.get(key) {
        None => Err(()),
        Some(v) => v.bool().ok_or(()),
    };
    let required = |key: &[u8]| match d.get(key) {
        None => Err(()),
        Some(v) => v.number().filter(|n| n.is_finite()).ok_or(()),
    };
    Ok(CgedBlock {
        version: required(b"Vrsn")?,
        brightness: required(b"Brgh")?,
        contrast: required(b"Cntr")?,
        means: required(b"means")?,
        lab: flag(b"Lab ")?,
        legacy: flag(b"useLegacy")?,
        auto: flag(b"Auto")?,
    })
}

/// A Brightness/Contrast layer from its `brit` block and, when the record
/// had one, its `CgEd` descriptor (the default mode keeps its settings
/// there and writes zeros to `brit`; Use Legacy has no `CgEd` in the
/// fixtures). Layers using Auto, Lab-only or a mean other than 127 are left
/// out (nothing checked how they apply).
fn brightness_contrast_adjustment(
    brit: Result<BritBlock, ()>,
    cged: Option<&Result<CgedBlock, ()>>,
    gray: bool,
) -> Result<Adjustment, Note> {
    let unreadable = || Note::Unreadable(Kind::BrightnessContrast);
    if gray {
        return Err(Note::Grayscale(Kind::BrightnessContrast));
    }
    let brit = brit.map_err(|()| unreadable())?;
    let mean = i16::from(aurora_core::BRIGHTNESS_CONTRAST_MEAN);
    let (brightness, contrast, legacy, options) = match cged {
        Some(Ok(c)) => {
            if (c.version - 1.0).abs() > f64::EPSILON {
                return Err(unreadable());
            }
            let whole = |v: f64| {
                (v.fract() == 0.0 && v.abs() <= 1_000.0)
                    .then_some(v as i16)
                    .ok_or_else(unreadable)
            };
            (
                whole(c.brightness)?,
                whole(c.contrast)?,
                c.legacy,
                c.lab || c.auto || whole(c.means)? != mean || brit.lab,
            )
        }
        Some(Err(())) => return Err(unreadable()),
        // A default-mode layer writes eight zero bytes here and keeps its
        // values in `CgEd`; all zeros without it is that shape with its
        // descriptor missing, not a legacy 0/0 layer (whose mean is 127).
        None if brit.brightness == 0 && brit.contrast == 0 && brit.mean == 0 && !brit.lab => {
            return Err(unreadable());
        }
        None => (
            brit.brightness,
            brit.contrast,
            true,
            brit.lab || brit.mean != mean,
        ),
    };
    if options {
        return Err(Note::BrightnessContrastOption);
    }
    aurora_core::BrightnessContrastParams::new(brightness, contrast, legacy)
        .map(Adjustment::BrightnessContrast)
        .map_err(|_| unreadable())
}

// ---------------------------------------------------------------------
// Channel Mixer and Selective Color (0.245.0)
// ---------------------------------------------------------------------

/// A `mixr` block: `u16` version (`1`), `u16` Monochrome (`0` or `1`), then
/// four rows of five `i16` (checked on `layers/channel-mixer.psd` and
/// `fill_adjustments.psd`, both the neutral mixer: rows `100,0,0,0,0`,
/// `0,100,0,0,0`, `0,0,100,0,0`, `0,0,0,100,0`, so the fourth source of a
/// row is the black of a CMYK document and the fifth value the constant, as
/// Adobe's spec says). Exactly 44 bytes.
/// A non-zero fourth source on an RGB row is left out as unreadable. **No fixture has Monochrome on**, so which row holds the Gray
/// mix is unverified (the first is assumed).
fn parse_mixer(data: &[u8]) -> Result<MixerBlock, ()> {
    // Exactly 44 bytes, as in every fixture: a longer block is something
    // this layout does not describe.
    if data.len() != 44 {
        return Err(());
    }
    let mut r = Reader::new(data);
    let unreadable = |_| ();
    if r.u16("mixer version").map_err(unreadable)? != 1 {
        return Err(());
    }
    let monochrome = match r.u16("mixer monochrome").map_err(unreadable)? {
        0 => false,
        1 => true,
        _ => return Err(()),
    };
    let mut rows = [[0_i16; 4]; 3];
    for row in &mut rows {
        let mut values = [0_i16; 5];
        for v in &mut values {
            *v = r.i16("mixer value").map_err(unreadable)?;
        }
        let [red, green, blue, fourth, constant] = values;
        // The fourth source is the black of a CMYK document; on an RGB row
        // every fixture holds 0, and anything else is not understood.
        if fourth != 0 {
            return Err(());
        }
        *row = [red, green, blue, constant];
    }
    Ok(MixerBlock { monochrome, rows })
}

/// A parsed block as Aurora's Channel Mixer (sources `-200..=200`,
/// constants `-100..=100`; anything else is left out, not repaired).
pub(super) fn channel_mixer_adjustment(block: &MixerBlock, gray: bool) -> Result<Adjustment, Note> {
    if gray {
        return Err(Note::Grayscale(Kind::ChannelMixer));
    }
    // Which row holds the Gray mix is in no fixture and not in psd-tools
    // (`psd/adjustments.py` `ChannelMixer.read`: version, monochrome, five
    // `i16`, the rest kept as `unknown`): a Monochrome mixer is left out.
    if block.monochrome {
        return Err(Note::Unverified(Kind::ChannelMixer));
    }
    aurora_core::ChannelMixerParams::new(block.rows.map(|r| r.map(f32::from)), block.monochrome)
        .map(Adjustment::ChannelMixer)
        .map_err(|_| Note::Unreadable(Kind::ChannelMixer))
}

/// A `selc` block: `u16` version (`1`), `u16` method (`0` Relative, `1`
/// Absolute), ten plates of four `i16` — 84 bytes, as in both fixtures
/// (all neutral, so the **plate order is unverified**: the first plate is
/// taken as unused and the next nine as the Photoshop menu order; a
/// non-zero first plate is left out rather than guessed at).
fn parse_selective(data: &[u8]) -> Result<SelectiveBlock, ()> {
    // Exactly 84 bytes, as in every fixture.
    if data.len() != 84 {
        return Err(());
    }
    let mut r = Reader::new(data);
    let unreadable = |_| ();
    if r.u16("selective version").map_err(unreadable)? != 1 {
        return Err(());
    }
    let absolute = match r.u16("selective method").map_err(unreadable)? {
        0 => false,
        1 => true,
        _ => return Err(()),
    };
    let mut plate = || -> Result<[i16; 4], ()> {
        let mut p = [0_i16; 4];
        for v in &mut p {
            *v = r.i16("selective value").map_err(unreadable)?;
        }
        Ok(p)
    };
    let unused = plate()?;
    let mut ranges = [[0_i16; 4]; 9];
    for range in &mut ranges {
        *range = plate()?;
    }
    Ok(SelectiveBlock {
        absolute,
        unused,
        ranges,
    })
}

/// A parsed block as Aurora's Selective Color (`-100..=100` sliders).
pub(super) fn selective_color_adjustment(
    block: &SelectiveBlock,
    gray: bool,
) -> Result<Adjustment, Note> {
    if gray {
        return Err(Note::Grayscale(Kind::SelectiveColor));
    }
    if block.unused != [0; 4] {
        return Err(Note::Unreadable(Kind::SelectiveColor));
    }
    let params = aurora_core::SelectiveColorParams::new(
        block.ranges.map(|r| r.map(f32::from)),
        block.absolute,
    )
    .map_err(|_| Note::Unreadable(Kind::SelectiveColor))?;
    // The order of the plates is in no fixture and not in psd-tools
    // (`psd/adjustments.py` `SelectiveColor.read`: version, method, ten
    // plates of four `i16`, no names): a layer that adjusts anything would
    // be applied to colours nobody has verified, so it is left out.
    if !params.is_identity() {
        return Err(Note::Unverified(Kind::SelectiveColor));
    }
    Ok(Adjustment::SelectiveColor(params))
}

// ---------------------------------------------------------------------
// Gradient Map and Color Lookup (0.248.0)
// ---------------------------------------------------------------------

/// The longest `grdm` or `clrL` block read. A real Gradient Map block is
/// under 300 bytes; 64 KiB allows over 3,000 stops, far beyond the cap.
const MAX_GRADIENT_BLOCK_BYTES: usize = 64 * 1024;

/// A colour stop as the file holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileColourStop {
    /// `0..=4096`.
    pub location: u32,
    /// Percent, `50` by default.
    pub midpoint: u32,
    /// The colour mode: `0` RGB.
    pub mode: u16,
    /// Four `u16` components; for RGB red, green, blue and a zero.
    pub colour: [u16; 4],
}

/// A transparency stop as the file holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileTransparencyStop {
    pub location: u32,
    pub midpoint: u32,
    /// `0..=255`.
    pub opacity: u16,
}

/// A parsed `grdm` block (0.248.0). Layout, checked on
/// `layers/gradient-map.psd`, the three `gradient-map-v3-*.psd` and
/// `fill_adjustments.psd` and against psd-tools 1.17.4's `GradientMap`:
/// `u16` version (`1`, or `3` which adds a four-character interpolation
/// method after the flags), `u8` Reverse, `u8` Dither, `method`, a unicode
/// name (`u32` count of UTF-16 units including a NUL), `u16` colour stop
/// count and per stop `u32` location (`0..=4096`), `u32` midpoint percent,
/// `u16` colour mode and four `u16` colour components and two pad bytes,
/// `u16` transparency stop count and per stop `u32` location, `u32`
/// midpoint, `u16` opacity (`0..=255`), then `u16` expansion (`2`), `u16`
/// smoothness (`4096` = 100 %), `u16` length (`32`), `u16` gradient mode (`0`
/// solid), `u32` random seed, `u16` show transparency, `u16` use vector
/// colour, `u32` roughness, `u16` colour model, two colours of four `u16`,
/// and two pad bytes. Colour components are `0..=65535` (`ffff` is full).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GradientBlock {
    pub reverse: bool,
    pub dither: bool,
    /// `Gcls` for a version 1 block.
    pub method: [u8; 4],
    pub colours: Vec<FileColourStop>,
    pub transparency: Vec<FileTransparencyStop>,
    pub smoothness: u16,
    /// `0` Solid, `1` Noise.
    pub gradient_mode: u16,
}

/// A parsed `clrL` block: `u16` version (`1`), `u32` data version (`16`) and
/// a descriptor. **Every fixture holds an empty descriptor** (no `lookupType`,
/// no name, no table); a non-empty one has never been seen, so only its
/// presence, its `Nm  ` text and its `lookupType` enum are read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LookupBlock {
    /// The descriptor holds no items at all.
    pub empty: bool,
    /// The `Nm  ` text, when there is one.
    pub name: Option<String>,
}

/// Why a Color Lookup layer is left out (it never becomes a layer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ColorLookupReason {
    /// An empty descriptor: no table was chosen.
    NoTable,
    /// A descriptor with a table or profile Aurora cannot apply.
    Table,
    Unreadable,
}

/// Why a Gradient Map layer is left out although its block is readable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum GradientReason {
    /// A colour interpolation method other than Classic (Linear,
    /// Perceptual or unknown): no fixture holds a picture it mapped.
    Method,
    /// A stop whose midpoint is not 50 %.
    Midpoint,
    /// A transparency stop below full opacity.
    Transparency,
    /// A colour stop not in RGB.
    ColourMode,
    /// A Noise gradient.
    Noise,
    /// More colour stops than Aurora allows.
    Stops,
}

fn parse_gradient_map(data: &[u8]) -> Result<GradientBlock, ()> {
    if data.len() > MAX_GRADIENT_BLOCK_BYTES {
        return Err(());
    }
    let mut r = Reader::new(data);
    let bad = |_| ();
    let version = r.u16("gradient version").map_err(bad)?;
    let reverse = flag(r.u8("gradient reverse").map_err(bad)?)?;
    let dither = flag(r.u8("gradient dither").map_err(bad)?)?;
    let method = match version {
        1 => *b"Gcls",
        3 => r.array::<4>("gradient method").map_err(bad)?,
        _ => return Err(()),
    };
    // A unicode string: its length is checked against the bytes left
    // before it is skipped.
    let units = usize::try_from(r.u32("gradient name length").map_err(bad)?).map_err(|_| ())?;
    r.skip(units.checked_mul(2).ok_or(())?, "gradient name")
        .map_err(bad)?;
    let colour_count = usize::from(r.u16("colour stop count").map_err(bad)?);
    // 20 bytes a stop: refuse a count the block cannot hold before reading.
    if colour_count.checked_mul(20).ok_or(())? > r.remaining() {
        return Err(());
    }
    let mut colours = Vec::with_capacity(colour_count);
    for _ in 0..colour_count {
        let location = r.u32("stop location").map_err(bad)?;
        let midpoint = r.u32("stop midpoint").map_err(bad)?;
        let mode = r.u16("stop mode").map_err(bad)?;
        let mut colour = [0_u16; 4];
        for c in &mut colour {
            *c = r.u16("stop colour").map_err(bad)?;
        }
        r.skip(2, "stop pad").map_err(bad)?;
        colours.push(FileColourStop {
            location,
            midpoint,
            mode,
            colour,
        });
    }
    let transparency_count = usize::from(r.u16("transparency stop count").map_err(bad)?);
    if transparency_count.checked_mul(10).ok_or(())? > r.remaining() {
        return Err(());
    }
    let mut transparency = Vec::with_capacity(transparency_count);
    for _ in 0..transparency_count {
        transparency.push(FileTransparencyStop {
            location: r.u32("transparency location").map_err(bad)?,
            midpoint: r.u32("transparency midpoint").map_err(bad)?,
            opacity: r.u16("transparency opacity").map_err(bad)?,
        });
    }
    // expansion, smoothness, length, mode, seed, show transparency, use
    // vector colour, roughness, colour model, two colours, two pad bytes.
    if r.u16("gradient expansion").map_err(bad)? != 2 {
        return Err(());
    }
    let smoothness = r.u16("gradient smoothness").map_err(bad)?;
    if r.u16("gradient length").map_err(bad)? != 32 {
        return Err(());
    }
    let gradient_mode = r.u16("gradient mode").map_err(bad)?;
    r.skip(4 + 2 + 2 + 4 + 2 + 16 + 2, "gradient settings")
        .map_err(bad)?;
    // Only the block's own padding may follow, as zeros.
    let tail = r.take(r.remaining(), "gradient tail").map_err(bad)?;
    if tail.len() > 3 || tail.iter().any(|&b| b != 0) {
        return Err(());
    }
    Ok(GradientBlock {
        reverse,
        dither,
        method,
        colours,
        transparency,
        smoothness,
        gradient_mode,
    })
}

/// A `0`/`1` flag byte; anything else is unreadable.
fn flag(v: u8) -> Result<bool, ()> {
    match v {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(()),
    }
}

/// A parsed block as Aurora's Gradient Map, or the reason it is left out.
/// Only what the model can hold exactly is imported: Classic interpolation,
/// default midpoints, opaque transparency stops, RGB colour stops, a Solid
/// gradient. **Nothing about the result is compared with Photoshop's
/// pixels** (no fixture holds a mapped picture).
pub(super) fn gradient_map_adjustment(
    block: &GradientBlock,
    gray: bool,
) -> Result<Adjustment, Note> {
    if gray {
        return Err(Note::Grayscale(Kind::GradientMap));
    }
    let unreadable = || Note::Unreadable(Kind::GradientMap);
    let left_out = |reason| Note::GradientLeftOut(reason);
    if block.colours.len() < aurora_core::GRADIENT_MAP_MIN_STOPS {
        return Err(unreadable());
    }
    if block.colours.len() > aurora_core::GRADIENT_MAP_MAX_STOPS {
        return Err(left_out(GradientReason::Stops));
    }
    if block.gradient_mode > 1 || block.smoothness > 4096 {
        return Err(unreadable());
    }
    if block.gradient_mode == 1 {
        return Err(left_out(GradientReason::Noise));
    }
    if &block.method != b"Gcls" {
        return Err(left_out(GradientReason::Method));
    }
    let mut stops = Vec::with_capacity(block.colours.len());
    for s in &block.colours {
        if s.location > 4096 || s.midpoint > 100 || s.colour[3] != 0 {
            return Err(unreadable());
        }
        if s.mode != 0 {
            return Err(left_out(GradientReason::ColourMode));
        }
        let [r, g, b, _] = s.colour;
        stops.push(aurora_core::GradientStop::new(
            f32::from(u16::try_from(s.location).map_err(|_| unreadable())?) / 4096.0,
            [r, g, b].map(|c| f32::from(c) / 65535.0),
        ));
    }
    if block.colours.iter().any(|s| s.midpoint != 50) {
        return Err(left_out(GradientReason::Midpoint));
    }
    if block.transparency.iter().any(|t| t.opacity > 255) {
        return Err(unreadable());
    }
    if block.transparency.iter().any(|t| t.opacity != 255) {
        return Err(left_out(GradientReason::Transparency));
    }
    aurora_core::GradientMapParams::new(stops, block.reverse, block.dither)
        .map(Adjustment::GradientMap)
        .map_err(|_| unreadable())
}

/// A `clrL` block: see [`LookupBlock`].
fn parse_color_lookup(data: &[u8]) -> Result<LookupBlock, ()> {
    if data.len() > MAX_GRADIENT_BLOCK_BYTES || data.len() < 2 {
        return Err(());
    }
    let mut r = Reader::new(data);
    if r.u16("lookup version").map_err(|_| ())? != 1 {
        return Err(());
    }
    let mut budget = ItemBudget {
        remaining: MAX_DESCRIPTOR_ITEMS,
    };
    let d = descriptor::parse_adjustment_block(data.get(2..).ok_or(())?, &mut budget)
        .map_err(|_| ())?;
    let name = match d.get(b"Nm  ") {
        Some(descriptor::Value::Text(t)) => Some(
            t.chars()
                .filter(|c| !c.is_control())
                .take(64)
                .collect::<String>(),
        )
        .filter(|t| !t.is_empty()),
        _ => None,
    };
    Ok(LookupBlock {
        empty: d.items.is_empty(),
        name,
    })
}

/// A parsed block as an adjustment, or the reason it is left out.
pub(super) fn adjustment(parsed: &Parsed, gray: bool) -> Result<Adjustment, Note> {
    match parsed {
        Parsed::Vibrance(b) => b
            .as_ref()
            .map_err(|()| Note::Unreadable(Kind::Vibrance))
            .and_then(|b| vibrance_adjustment(b, gray)),
        Parsed::Exposure(b) => b
            .as_ref()
            .map_err(|()| Note::Unreadable(Kind::Exposure))
            .and_then(|b| exposure_adjustment(b, gray)),
        Parsed::PhotoFilter(b) => b
            .as_ref()
            .map_err(|()| Note::Unreadable(Kind::PhotoFilter))
            .and_then(|b| photo_filter_adjustment(b, gray)),
        Parsed::BlackWhite(b) => b
            .as_ref()
            .map_err(|()| Note::Unreadable(Kind::BlackWhite))
            .and_then(|b| black_white_adjustment(b, gray)),
        Parsed::ColorBalance(b) => b
            .as_ref()
            .map_err(|()| Note::Unreadable(Kind::ColorBalance))
            .and_then(|b| color_balance_adjustment(b, gray)),
        Parsed::Invert(b) => match (b, gray) {
            (_, true) => Err(Note::Grayscale(Kind::Invert)),
            (Err(()), _) => Err(Note::Unreadable(Kind::Invert)),
            (Ok(()), false) => Ok(Adjustment::Invert),
        },
        Parsed::Threshold(b) => match (b, gray) {
            (_, true) => Err(Note::Grayscale(Kind::Threshold)),
            (Err(()), _) => Err(Note::Unreadable(Kind::Threshold)),
            (Ok(level), false) => u8::try_from(*level)
                .map_err(|_| ())
                .and_then(|l| aurora_core::ThresholdParams::new(l).map_err(|_| ()))
                .map(Adjustment::Threshold)
                .map_err(|()| Note::Unreadable(Kind::Threshold)),
        },
        Parsed::Posterize(b) => match (b, gray) {
            (_, true) => Err(Note::Grayscale(Kind::Posterize)),
            (Err(()), _) => Err(Note::Unreadable(Kind::Posterize)),
            (Ok(levels), false) => u8::try_from(*levels)
                .map_err(|_| ())
                .and_then(|l| aurora_core::PosterizeParams::new(l).map_err(|_| ()))
                .map(Adjustment::Posterize)
                .map_err(|()| Note::Unreadable(Kind::Posterize)),
        },
        Parsed::ChannelMixer(b) => b
            .as_ref()
            .map_err(|()| Note::Unreadable(Kind::ChannelMixer))
            .and_then(|b| channel_mixer_adjustment(b, gray)),
        Parsed::SelectiveColor(b) => b
            .as_ref()
            .map_err(|()| Note::Unreadable(Kind::SelectiveColor))
            .and_then(|b| selective_color_adjustment(b, gray)),
        Parsed::Brightness(brit, cged) => {
            brightness_contrast_adjustment(*brit, cged.as_ref(), gray)
        }
        Parsed::GradientMap(b) => b
            .as_ref()
            .map_err(|()| Note::Unreadable(Kind::GradientMap))
            .and_then(|b| gradient_map_adjustment(b, gray)),
        Parsed::ColorLookup(b) => Err(Note::ColorLookup(match b {
            Err(()) => ColorLookupReason::Unreadable,
            Ok(b) if b.empty => ColorLookupReason::NoTable,
            Ok(_) => ColorLookupReason::Table,
        })),
    }
}

/// The Photo Filter's Lab colour for the writer's value, in the file's
/// units: `(L * 100, a * 100, b * 100)` rounded and clamped to what `u16`
/// and `i16` hold.
pub(super) fn photo_filter_components(rgb: [f32; 3]) -> (u16, i16, i16) {
    let (l, a, b) = super::lab::srgb_to_lab(rgb);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped
    let l = (l * 100.0).round().clamp(0.0, 10_000.0) as u16;
    #[allow(clippy::cast_possible_truncation)] // clamped
    let ab = |v: f64| (v * 100.0).round().clamp(-12_800.0, 12_700.0) as i16;
    (l, ab(a), ab(b))
}
