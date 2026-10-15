//! Adjustment layers for the PSD writer (0.205.0): the tagged blocks
//! `curv`, `levl` and `hue2`, the exact inverse of the layouts the reader
//! parses ([`super::parse_curv`], [`super::parse_levl`],
//! [`super::parse_hue2`]).
//!
//! An adjustment layer is written the way Photoshop writes one (measured
//! with psd-tools on the corpus's `curves_rgb.psd`, `levels_rgb.psd` and
//! `huesaturation_*.psd`): an empty rectangle, the four colour channels
//! empty (a compression field and nothing else, like a group's records),
//! the user mask as for any layer, and the adjustment's own tagged block
//! first among the record's blocks.
//!
//! * `curv`: `0u8`, version `1`, a `u32` bitmap of the channels written
//!   (`0` composite, `1..=3` red, green, blue; low bit first), for each a
//!   `u16` count then `(output, input)` `u16` pairs in 0..=255, then the
//!   `Crv ` extension (version `4`, a `u32` count, per curve a `u16`
//!   channel and the same point list), padded with zeros to a multiple of
//!   four. The composite is written when it is not the identity, or when
//!   no channel curve is written, so a block always holds a curve.
//! * `levl`: version `2`, the 29 records of five `u16`
//!   `(input floor, input ceiling, output floor, output ceiling,
//!   gamma * 100)` — master, red, green, blue, then 25 neutral records —
//!   then Photoshop's `Lvls` extension (version `3`, count 62, 33 more
//!   neutral records), padded to four.
//! * `hue2`: version `2`, the Colorize byte and a pad byte, the Colorize
//!   `(hue, saturation, lightness)` and the master triple as `i16`, then
//!   six records of four edges and a triple, reds first. The Colorize hue
//!   is stored signed as Photoshop does (a hue above 180 is written minus
//!   360); the reader folds it back into 0..360.
//!
//! * `vibA` (0.241.0): a descriptor block (`u32` version 16, then the
//!   action descriptor), the keys `vibrance` and `Strt`, both `long`,
//!   each left out at `0` as Photoshop leaves them out
//!   (`layers/vibrance.psd`'s neutral block is an empty descriptor).
//! * `expA`: `u16` version `1`, three `f32` (exposure, offset, gamma),
//!   padded to 16 bytes. Exact: Aurora's values are `f32` already.
//! * `phfl`: version `2`, colour space `7` (Lab), `(L, a, b) * 100` as
//!   `u16, i16, i16` and a zero, the density as a `u32` percent, the
//!   preserve-luminosity byte (the layout of `layers/photo-filter.psd`).
//!   The sRGB colour is converted to Lab with D50 white
//!   ([`super::lab`]).
//! * `blwh`: a descriptor block with the ten items Photoshop writes
//!   (`layers/black-and-white.psd`): six `long` sliders, `useTint`, the
//!   `tintColor` `RGBC` object of three `doub` 0..255, `bwPresetKind`
//!   (written as the observed `1`) and an empty `blackAndWhitePresetFileName`.
//!   The tint colour is the HSL colour of the tint hue and saturation at
//!   lightness 0.5, as `doub` (Photoshop itself stores fractions there:
//!   `225.00045776367188`).
//! * `blnc`: nine `i16`, the preserve-luminosity byte and a pad byte.
//! * `mixr` / `selc` (0.245.0, revised): a Monochrome mixer and a Selective
//!   Color that adjusts anything are **refused** (the Gray row and the plate
//!   order are unverified); a neutral Selective Color of either method and a
//!   non-Monochrome mixer are written, the neutral ones byte for byte as
//!   Photoshop writes them.
//! * `mixr` (0.245.0): version `1`, the Monochrome `u16`, four rows of five
//!   `i16` (red, green, blue, the CMYK-only fourth source, constant): the
//!   three rows of the model with a zero fourth source, then the fourth row
//!   `0,0,0,100,0` as in the neutral fixtures. 44 bytes. Monochrome writes
//!   the Gray mix in the first row (unverified).
//! * `selc`: version `1`, the method `u16`, ten plates of four `i16`: an
//!   unused zero plate, then the nine ranges in the model's order. 84 bytes.
//!
//! Aurora's model is finer than the file: curve points are 8-bit levels in
//! the file, Levels values are 8-bit levels with a gamma in hundredths,
//! Hue/Saturation values are whole numbers. Each value is rounded to
//! nearest and a layer for which that changed anything is counted in the
//! save report ([`Rounding`]). A layer is refused only when the rounded
//! values would not read back: two curve points that become the same level
//! are merged (reported), but a curve that is left with one point, a
//! Levels record whose input range collapses or a colour range whose edges
//! no longer validate cannot be written.
//!
//! The reader decides what a block means, so the adjustment a file will
//! read back as ([`EncodedAdjustment::readback`]) is obtained by running
//! the reader's own parser over the written bytes; the verifier compares
//! the file with that, and the unit tests compare it with the original.
//!
//! Checked with Aurora's own reader and psd-tools; **never opened in
//! Photoshop**.

use super::read_adjust;
use aurora_core::{
    CurvesParams, DEFAULT_HUE_RANGE_EDGES, HUE_RANGE_COUNT, HueRange, HueSaturationParams,
    LevelsChannel, ToneCurve,
};
use aurora_doc::Adjustment;

/// How many records `levl` holds before its `Lvls` extension.
const LEVL_RECORDS: usize = 29;

/// The record count Photoshop writes in the `Lvls` extension.
const LVLS_TOTAL: u16 = 62;

/// What had to change for the file to hold an adjustment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // four independent facts about one layer
pub(super) struct Rounding {
    /// A value was rounded to the file's resolution.
    pub rounded: bool,
    /// Curve points that landed on the same level were merged.
    pub merged_points: bool,
    /// A colour range with custom edges that adjusts nothing was written
    /// with Photoshop's standard edges.
    pub ranges_reset: bool,
    /// Colorize is on and a colour range adjusts something: the ranges are
    /// written, but Aurora's reader drops them again (Colorize has none).
    pub colorize_ranges: bool,
}

/// A second tagged block of an adjustment record: key and data.
pub(super) type ExtraBlock = ([u8; 4], Vec<u8>);

/// One adjustment's tagged block, ready to write.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EncodedAdjustment {
    /// `curv`, `levl` or `hue2`.
    pub key: [u8; 4],
    /// The block's data, zero-padded to a multiple of four.
    pub data: Vec<u8>,
    /// A second block written after it (0.244.0): see
    /// [`super::PsdExpectedAdjustment::extra`].
    pub extra: Option<ExtraBlock>,
    /// What the reader makes of the block (and its extra one).
    pub readback: Adjustment,
    pub rounding: Rounding,
}

/// Whether `key` is one of the eight blocks this module writes.
pub(super) fn is_adjustment_key(key: [u8; 4]) -> bool {
    matches!(&key, b"curv" | b"levl" | b"hue2" | b"CgEd") || read_adjust::is_key(key)
}

fn pad4(data: &mut Vec<u8>) {
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
}

/// `0.0..=1.0` to a level `0..=scale`, rounded to nearest.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped first
fn to_level(value: f32, scale: f32) -> u16 {
    (value.clamp(0.0, 1.0) * scale).round() as u16
}

/// A gamma in hundredths (validated within 0.10..=9.99).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 10..=999
fn to_hundredths(gamma: f32) -> u16 {
    (gamma.clamp(0.0, 600.0) * 100.0).round() as u16
}

/// Whether `level / scale` is exactly the value it came from.
fn level_is_exact(value: f32, level: u16, scale: f32) -> bool {
    (f32::from(level) / scale).to_bits() == value.to_bits()
}

/// A whole number of a validated value (all within a few hundred).
#[allow(clippy::cast_possible_truncation)] // |v| <= 360
fn whole(value: f32) -> i16 {
    value.round() as i16
}

/// Encodes `adjustment` as its tagged block.
///
/// # Errors
///
/// The reason the file cannot hold it: the rounded values do not read back.
pub(super) fn encode_adjustment(
    adjustment: &Adjustment,
) -> Result<EncodedAdjustment, &'static str> {
    // The file layouts of a Monochrome mixer (which row is the Gray mix) and
    // of a Selective Color that adjusts anything (the order of the ten
    // plates) are not documented by any fixture or by psd-tools: writing
    // them could make Photoshop apply a client's layer to the wrong colours.
    match adjustment {
        Adjustment::ChannelMixer(p) if p.monochrome() => {
            return Err(
                "its Monochrome Gray mix has no verified place in the file's rows (no Photoshop file \
                 documents it); save as .aur to keep it",
            );
        }
        Adjustment::SelectiveColor(p) if !p.is_identity() => {
            return Err(
                "the order of the file's ten colour plates is not verified, so its adjustments \
                 could land on the wrong colours; save as .aur to keep it",
            );
        }
        _ => {}
    }
    let mut rounding = Rounding::default();
    let mut extra = None;
    let (key, mut data) = match adjustment {
        Adjustment::Curves(params) => (*b"curv", encode_curv(params, &mut rounding)?),
        Adjustment::Levels(params) => (*b"levl", encode_levl(params, &mut rounding)),
        Adjustment::Vibrance(params) => (*b"vibA", encode_vibrance(*params, &mut rounding)),
        Adjustment::Exposure(params) => (*b"expA", encode_exposure(params)),
        Adjustment::PhotoFilter(params) => (*b"phfl", encode_photo_filter(params, &mut rounding)),
        Adjustment::BlackWhite(params) => (*b"blwh", encode_black_white(params, &mut rounding)),
        Adjustment::ColorBalance(params) => (*b"blnc", encode_color_balance(params, &mut rounding)),
        Adjustment::ChannelMixer(params) => (*b"mixr", encode_channel_mixer(params, &mut rounding)),
        Adjustment::SelectiveColor(params) => {
            (*b"selc", encode_selective_color(params, &mut rounding))
        }
        Adjustment::Invert => (*b"nvrt", Vec::new()),
        Adjustment::Threshold(params) => (*b"thrs", encode_level(u16::from(params.level()))),
        Adjustment::Posterize(params) => (*b"post", encode_level(u16::from(params.levels()))),
        Adjustment::BrightnessContrast(params) => {
            let (brit, cged) = encode_brightness_contrast(*params);
            extra = cged;
            (*b"brit", brit)
        }
        Adjustment::HueSaturation(_) | Adjustment::HueSaturationRanges(_) => {
            let Some(params) = adjustment.hue_saturation() else {
                return Err("its Hue/Saturation settings are unreadable");
            };
            rounding.colorize_ranges = params.base().colorize() && !params.ranges_adjust_nothing();
            (
                *b"hue2",
                encode_hue2(params.base(), params.ranges(), &mut rounding),
            )
        }
    };
    pad4(&mut data);
    if let Some((_, extra_data)) = &mut extra {
        pad4(extra_data);
    }
    let readback = read_back(key, &data, extra.as_ref().map(|(_, d)| d.as_slice()))?;
    Ok(EncodedAdjustment {
        key,
        data,
        extra,
        readback,
        rounding,
    })
}

/// The adjustment the reader makes of a block, or why it would refuse.
fn read_back(key: [u8; 4], data: &[u8], extra: Option<&[u8]>) -> Result<Adjustment, &'static str> {
    match &key {
        b"brit" => {
            let Some(mut parsed) = read_adjust::parse_block(key, data) else {
                return Err("its settings do not read back");
            };
            if let Some(extra) = extra {
                read_adjust::attach_cged(&mut parsed, extra);
            }
            read_adjust::adjustment(&parsed, false)
                .map_err(|_| "its values are not valid once written to the file")
        }
        b"curv" => {
            let block = super::parse_curv(data).map_err(|_| "its curves do not read back")?;
            super::curves_params(&block, false)
                .map(Adjustment::Curves)
                .map_err(|_| "its curves do not read back")
        }
        b"levl" => {
            let block = super::parse_levl(data).map_err(|()| "its levels do not read back")?;
            super::levels_params(&block, false)
                .map(Adjustment::Levels)
                .map_err(|_| "its Levels values are not valid once rounded to 8-bit levels")
        }
        k if read_adjust::is_key(*k) => read_back_new(*k, data),
        _ => {
            let block = super::parse_hue2(data).map_err(|()| "its settings do not read back")?;
            super::hue_saturation_adjustment(&block, false).map_err(|_| {
                "its values are not valid once rounded to whole numbers (a colour range's \
                 edges wrap more than once)"
            })
        }
    }
}

// ---------------------------------------------------------------------
// Curves
// ---------------------------------------------------------------------

/// One curve's points as `(output, input)` levels; points on the same
/// input level are merged (the first stays).
fn curve_levels(curve: &ToneCurve, rounding: &mut Rounding) -> Vec<(u16, u16)> {
    let mut out: Vec<(u16, u16)> = Vec::with_capacity(curve.points().len());
    for point in curve.points() {
        let input = to_level(point.x, 255.0);
        let output = to_level(point.y, 255.0);
        if !level_is_exact(point.x, input, 255.0) || !level_is_exact(point.y, output, 255.0) {
            rounding.rounded = true;
        }
        if out.last().is_some_and(|&(_, last)| last == input) {
            rounding.merged_points = true;
            continue;
        }
        out.push((output, input));
    }
    out
}

fn push_u16(data: &mut Vec<u8>, v: u16) {
    data.extend_from_slice(&v.to_be_bytes());
}

fn push_points(data: &mut Vec<u8>, points: &[(u16, u16)]) {
    push_u16(data, u16::try_from(points.len()).unwrap_or(u16::MAX));
    for &(output, input) in points {
        push_u16(data, output);
        push_u16(data, input);
    }
}

fn encode_curv(params: &CurvesParams, rounding: &mut Rounding) -> Result<Vec<u8>, &'static str> {
    let mut curves: Vec<(u16, Vec<(u16, u16)>)> = Vec::new();
    let channel_curves = [(1u16, &params.red), (2, &params.green), (3, &params.blue)];
    let any_channel = channel_curves.iter().any(|(_, c)| c.is_some());
    if !params.composite.is_identity() || !any_channel {
        curves.push((0, curve_levels(&params.composite, rounding)));
    }
    for (channel, curve) in channel_curves {
        if let Some(curve) = curve {
            curves.push((channel, curve_levels(curve, rounding)));
        }
    }
    if curves.iter().any(|(_, points)| points.len() < 2) {
        return Err("a curve's points fall on fewer than two different 8-bit levels once rounded");
    }
    let bitmap = curves.iter().fold(0u32, |bits, (c, _)| bits | (1 << c));
    let mut data = vec![0u8];
    push_u16(&mut data, 1);
    data.extend_from_slice(&bitmap.to_be_bytes());
    for (_, points) in &curves {
        push_points(&mut data, points);
    }
    data.extend_from_slice(b"Crv ");
    push_u16(&mut data, 4);
    data.extend_from_slice(&u32::try_from(curves.len()).unwrap_or(0).to_be_bytes());
    for (channel, points) in &curves {
        push_u16(&mut data, *channel);
        push_points(&mut data, points);
    }
    Ok(data)
}

// ---------------------------------------------------------------------
// Levels
// ---------------------------------------------------------------------

/// A record as the file holds it:
/// `(input floor, input ceiling, output floor, output ceiling, gamma * 100)`.
fn levl_record(channel: &LevelsChannel, rounding: &mut Rounding) -> [u16; 5] {
    let ib = to_level(channel.input_black(), 255.0);
    let iw = to_level(channel.input_white(), 255.0);
    let ob = to_level(channel.output_black(), 255.0);
    let ow = to_level(channel.output_white(), 255.0);
    let gamma = to_hundredths(channel.gamma());
    let exact = level_is_exact(channel.input_black(), ib, 255.0)
        && level_is_exact(channel.input_white(), iw, 255.0)
        && level_is_exact(channel.output_black(), ob, 255.0)
        && level_is_exact(channel.output_white(), ow, 255.0)
        && level_is_exact(channel.gamma(), gamma, 100.0);
    if !exact {
        rounding.rounded = true;
    }
    [ib, iw, ob, ow, gamma]
}

fn encode_levl(params: &aurora_core::LevelsParams, rounding: &mut Rounding) -> Vec<u8> {
    const NEUTRAL: [u16; 5] = [0, 255, 0, 255, 100];
    let mut records: Vec<[u16; 5]> = [&params.master, &params.red, &params.green, &params.blue]
        .into_iter()
        .map(|c| levl_record(c, rounding))
        .collect();
    records.resize(usize::from(LVLS_TOTAL), NEUTRAL);
    let mut data = Vec::new();
    push_u16(&mut data, 2);
    for record in records.iter().take(LEVL_RECORDS) {
        for &v in record {
            push_u16(&mut data, v);
        }
    }
    data.extend_from_slice(b"Lvls");
    push_u16(&mut data, 3);
    push_u16(&mut data, LVLS_TOTAL);
    for record in records.iter().skip(LEVL_RECORDS) {
        for &v in record {
            push_u16(&mut data, v);
        }
    }
    data
}

// ---------------------------------------------------------------------
// Hue/Saturation
// ---------------------------------------------------------------------

/// A whole number of `v`, noting when it changed.
fn tracked(rounding: &mut Rounding, v: f32) -> i16 {
    let w = whole(v);
    if f32::from(w).to_bits() != v.to_bits() {
        rounding.rounded = true;
    }
    w
}

fn push_i16(data: &mut Vec<u8>, v: i16) {
    data.extend_from_slice(&v.to_be_bytes());
}

fn encode_hue2(
    base: &HueSaturationParams,
    ranges: &[HueRange; HUE_RANGE_COUNT],
    rounding: &mut Rounding,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(104);
    push_u16(&mut data, 2);
    data.push(u8::from(base.colorize()));
    data.push(0);
    // Photoshop stores the Colorize hue signed.
    let colorize_hue = {
        let w = tracked(rounding, base.colorize_hue());
        if w > 180 { w - 360 } else { w }
    };
    push_i16(&mut data, colorize_hue);
    push_i16(&mut data, tracked(rounding, base.colorize_saturation()));
    push_i16(&mut data, tracked(rounding, base.colorize_lightness()));
    push_i16(&mut data, tracked(rounding, base.hue()));
    push_i16(&mut data, tracked(rounding, base.saturation()));
    push_i16(&mut data, tracked(rounding, base.lightness()));
    for (index, range) in ranges.iter().enumerate() {
        if range.adjusts_nothing() {
            // The reader (and Photoshop's neutral state) use the standard
            // edges for a range that does nothing.
            let standard = DEFAULT_HUE_RANGE_EDGES
                .get(index)
                .copied()
                .unwrap_or_default();
            // Both are small whole numbers held in `f32`: exact.
            #[allow(clippy::float_cmp)]
            let custom = range.edges() != standard;
            if custom {
                rounding.ranges_reset = true;
            }
            for edge in standard {
                push_i16(&mut data, whole(edge));
            }
            for _ in 0..3 {
                push_i16(&mut data, 0);
            }
            continue;
        }
        for edge in range.edges() {
            push_i16(&mut data, tracked(rounding, edge));
        }
        push_i16(&mut data, tracked(rounding, range.hue()));
        push_i16(&mut data, tracked(rounding, range.saturation()));
        push_i16(&mut data, tracked(rounding, range.lightness()));
    }
    data
}

// ---------------------------------------------------------------------
// Vibrance, Exposure, Photo Filter, Black & White, Color Balance (0.241.0)
// ---------------------------------------------------------------------

/// The adjustment the reader makes of one of the five new blocks.
fn read_back_new(key: [u8; 4], data: &[u8]) -> Result<Adjustment, &'static str> {
    let Some(parsed) = read_adjust::parse_block(key, data) else {
        return Err("its settings do not read back");
    };
    read_adjust::adjustment(&parsed, false)
        .map_err(|_| "its values are not valid once rounded to what the file holds")
}

fn push_u32(data: &mut Vec<u8>, v: u32) {
    data.extend_from_slice(&v.to_be_bytes());
}

/// The start of an action descriptor block: version 16, an empty (single
/// NUL) name, class `null`, and `count` items.
fn descriptor_start(count: u32) -> Vec<u8> {
    let mut data = Vec::new();
    push_u32(&mut data, 16);
    push_u32(&mut data, 1);
    push_u16(&mut data, 0);
    descriptor_class_and_count(&mut data, *b"null", count);
    data
}

fn descriptor_class_and_count(data: &mut Vec<u8>, class: [u8; 4], count: u32) {
    push_u32(data, 0);
    data.extend_from_slice(&class);
    push_u32(data, count);
}

/// A key: four characters are written as length 0 plus the four, longer
/// ones with their length (Photoshop's own convention).
fn push_key(data: &mut Vec<u8>, key: &[u8]) {
    if key.len() == 4 {
        push_u32(data, 0);
    } else {
        push_u32(data, u32::try_from(key.len()).unwrap_or(0));
    }
    data.extend_from_slice(key);
}

fn push_long(data: &mut Vec<u8>, key: &[u8], v: i32) {
    push_key(data, key);
    data.extend_from_slice(b"long");
    data.extend_from_slice(&v.to_be_bytes());
}

fn push_double(data: &mut Vec<u8>, key: &[u8], v: f64) {
    push_key(data, key);
    data.extend_from_slice(b"doub");
    data.extend_from_slice(&v.to_be_bytes());
}

/// A whole number of `v` as the `i32` a `long` holds, noting a change.
#[allow(clippy::cast_possible_truncation)] // validated values are within a few hundred
fn long_of(rounding: &mut Rounding, v: f32) -> i32 {
    let w = v.round() as i32;
    #[allow(clippy::cast_precision_loss)] // |w| is tiny
    if (w as f32).to_bits() != v.to_bits() {
        rounding.rounded = true;
    }
    w
}

fn encode_vibrance(params: aurora_core::VibranceParams, rounding: &mut Rounding) -> Vec<u8> {
    let items = [
        (&b"vibrance"[..], long_of(rounding, params.vibrance())),
        (&b"Strt"[..], long_of(rounding, params.saturation())),
    ];
    let written: Vec<_> = items.iter().filter(|(_, v)| *v != 0).collect();
    let mut data = descriptor_start(u32::try_from(written.len()).unwrap_or(0));
    for (key, value) in written {
        push_long(&mut data, key, *value);
    }
    data
}

fn encode_exposure(params: &aurora_core::ExposureParams) -> Vec<u8> {
    let mut data = Vec::with_capacity(16);
    push_u16(&mut data, 1);
    for v in [params.exposure(), params.offset(), params.gamma()] {
        data.extend_from_slice(&v.to_be_bytes());
    }
    data
}

fn encode_photo_filter(
    params: &aurora_core::PhotoFilterParams,
    rounding: &mut Rounding,
) -> Vec<u8> {
    let (l, a, b) = read_adjust::photo_filter_components(params.colour());
    let mut data = Vec::with_capacity(20);
    push_u16(&mut data, 2);
    push_u16(&mut data, 7);
    push_u16(&mut data, l);
    push_i16(&mut data, a);
    push_i16(&mut data, b);
    push_u16(&mut data, 0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 1..=100
    let density = params.density().round().clamp(1.0, 100.0) as u32;
    #[allow(clippy::cast_precision_loss)] // 1..=100
    if (density as f32).to_bits() != params.density().to_bits() {
        rounding.rounded = true;
    }
    push_u32(&mut data, density);
    data.push(u8::from(params.preserve_luminosity()));
    // The colour is Lab in hundredths: rounded when the file's colour is
    // more than a third of an 8-bit level from the layer's.
    let back = super::lab::lab_to_srgb(
        f64::from(l) / 100.0,
        f64::from(a) / 100.0,
        f64::from(b) / 100.0,
    );
    if back
        .iter()
        .zip(params.colour())
        .any(|(&got, want)| (got - want).abs() > COLOUR_TOLERANCE)
    {
        rounding.rounded = true;
    }
    data
}

/// A colour that returns within this of its value counts as unchanged
/// (a third of an 8-bit level; Lab hundredths move it by about `1e-4`).
const COLOUR_TOLERANCE: f32 = 1.0 / 765.0;

/// A tint hue and saturation that return within this are unchanged.
const TINT_TOLERANCE: f32 = 0.01;

fn encode_black_white(params: &aurora_core::BlackWhiteParams, rounding: &mut Rounding) -> Vec<u8> {
    let mut data = descriptor_start(10);
    for (key, weight) in [&b"Rd  "[..], b"Yllw", b"Grn ", b"Cyn ", b"Bl  ", b"Mgnt"]
        .into_iter()
        .zip(params.weights())
    {
        push_long(&mut data, key, long_of(rounding, weight));
    }
    push_key(&mut data, b"useTint");
    data.extend_from_slice(b"bool");
    data.push(u8::from(params.tint()));
    let rgb = read_adjust::tint_rgb(params.tint_hue(), params.tint_saturation());
    push_key(&mut data, b"tintColor");
    data.extend_from_slice(b"Objc");
    push_u32(&mut data, 1);
    push_u16(&mut data, 0);
    descriptor_class_and_count(&mut data, *b"RGBC", 3);
    for (key, v) in [&b"Rd  "[..], b"Grn ", b"Bl  "].into_iter().zip(rgb) {
        push_double(&mut data, key, v);
    }
    push_long(&mut data, b"bwPresetKind", 1);
    push_key(&mut data, b"blackAndWhitePresetFileName");
    data.extend_from_slice(b"TEXT");
    push_u32(&mut data, 1);
    push_u16(&mut data, 0);
    let (hue, saturation) = read_adjust::tint_hue_saturation(rgb);
    let hue_matters = params.tint_saturation() > 0.0;
    if (saturation - params.tint_saturation()).abs() > TINT_TOLERANCE
        || (hue_matters && (hue - params.tint_hue()).abs() > TINT_TOLERANCE)
    {
        rounding.rounded = true;
    }
    data
}

/// A `thrs` or `post` block: the level as a `u16` and two zero bytes
/// (`00e80000` in `adjustments/threshold_rgb.psd`).
fn encode_level(level: u16) -> Vec<u8> {
    let mut data = Vec::with_capacity(4);
    push_u16(&mut data, level);
    push_u16(&mut data, 0);
    data
}

/// The `brit` block and, for the default mode, the `CgEd` descriptor.
///
/// Use Legacy: `brit` holds `i16` brightness, contrast, the mean 127, and two
/// zero bytes (`0014 0039 007f 0000` in `brightnesscontrast_legacy_rgb.psd`).
/// The default mode: `brit` is eight zero bytes and the settings live in a
/// `CgEd` descriptor of seven items (`Vrsn` 1, `Brgh`, `Cntr`, `means` 127,
/// `Lab ` false, `useLegacy` false, `Auto` false), byte for byte what
/// `brightnesscontrast_rgb.psd` holds.
fn encode_brightness_contrast(
    params: aurora_core::BrightnessContrastParams,
) -> (Vec<u8>, Option<ExtraBlock>) {
    let mut brit = Vec::with_capacity(8);
    if params.legacy() {
        push_i16(&mut brit, params.brightness());
        push_i16(&mut brit, params.contrast());
        push_i16(&mut brit, i16::from(aurora_core::BRIGHTNESS_CONTRAST_MEAN));
        brit.extend_from_slice(&[0, 0]);
        return (brit, None);
    }
    brit.extend_from_slice(&[0; 8]);
    let mut cged = descriptor_start(7);
    push_long(&mut cged, b"Vrsn", 1);
    push_long(&mut cged, b"Brgh", i32::from(params.brightness()));
    push_long(&mut cged, b"Cntr", i32::from(params.contrast()));
    push_long(
        &mut cged,
        b"means",
        i32::from(aurora_core::BRIGHTNESS_CONTRAST_MEAN),
    );
    for key in [&b"Lab "[..], b"useLegacy", b"Auto"] {
        push_key(&mut cged, key);
        cged.extend_from_slice(b"bool");
        cged.push(0);
    }
    (brit, Some((*b"CgEd", cged)))
}

fn encode_color_balance(
    params: &aurora_core::ColorBalanceParams,
    rounding: &mut Rounding,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(20);
    for v in params.ranges().iter().flatten() {
        #[allow(clippy::cast_possible_truncation)] // -100..=100
        let w = v.round() as i16;
        if f32::from(w).to_bits() != v.to_bits() {
            rounding.rounded = true;
        }
        push_i16(&mut data, w);
    }
    data.push(u8::from(params.preserve_luminosity()));
    data
}

/// A whole percent of a validated slider; `rounded` is set when that
/// changed the value.
#[allow(clippy::cast_possible_truncation)] // |v| <= 200
fn whole_percent(v: f32, rounding: &mut Rounding) -> i16 {
    let w = v.round() as i16;
    if (f32::from(w) - v).abs() > 0.0 {
        rounding.rounded = true;
    }
    w
}

pub(super) fn encode_channel_mixer(
    params: &aurora_core::ChannelMixerParams,
    rounding: &mut Rounding,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(44);
    push_u16(&mut data, 1);
    push_u16(&mut data, u16::from(params.monochrome()));
    for row in params.rows() {
        let [red, green, blue, constant] = row;
        for v in [red, green, blue] {
            push_i16(&mut data, whole_percent(v, rounding));
        }
        push_i16(&mut data, 0);
        push_i16(&mut data, whole_percent(constant, rounding));
    }
    for v in [0, 0, 0, 100, 0] {
        push_i16(&mut data, v);
    }
    data
}

pub(super) fn encode_selective_color(
    params: &aurora_core::SelectiveColorParams,
    rounding: &mut Rounding,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(84);
    push_u16(&mut data, 1);
    push_u16(&mut data, u16::from(params.absolute()));
    for _ in 0..4 {
        push_i16(&mut data, 0);
    }
    for v in params.ranges().iter().flatten() {
        push_i16(&mut data, whole_percent(*v, rounding));
    }
    data
}
