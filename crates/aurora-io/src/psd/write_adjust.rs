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
pub(super) struct Rounding {
    /// A value was rounded to the file's resolution.
    pub rounded: bool,
    /// Curve points that landed on the same level were merged.
    pub merged_points: bool,
    /// A colour range with custom edges that adjusts nothing was written
    /// with Photoshop's standard edges.
    pub ranges_reset: bool,
}

/// One adjustment's tagged block, ready to write.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EncodedAdjustment {
    /// `curv`, `levl` or `hue2`.
    pub key: [u8; 4],
    /// The block's data, zero-padded to a multiple of four.
    pub data: Vec<u8>,
    /// What the reader makes of `data`.
    pub readback: Adjustment,
    pub rounding: Rounding,
}

/// Whether `key` is one of the three blocks this module writes.
pub(super) fn is_adjustment_key(key: &[u8; 4]) -> bool {
    matches!(key, b"curv" | b"levl" | b"hue2")
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
    let mut rounding = Rounding::default();
    let (key, mut data) = match adjustment {
        Adjustment::Curves(params) => (*b"curv", encode_curv(params, &mut rounding)?),
        Adjustment::Levels(params) => (*b"levl", encode_levl(params, &mut rounding)),
        Adjustment::HueSaturation(_) | Adjustment::HueSaturationRanges(_) => {
            let Some(params) = adjustment.hue_saturation() else {
                return Err("its Hue/Saturation settings are unreadable");
            };
            (
                *b"hue2",
                encode_hue2(params.base(), params.ranges(), &mut rounding),
            )
        }
    };
    pad4(&mut data);
    let readback = read_back(key, &data)?;
    Ok(EncodedAdjustment {
        key,
        data,
        readback,
        rounding,
    })
}

/// The adjustment the reader makes of a block, or why it would refuse.
fn read_back(key: [u8; 4], data: &[u8]) -> Result<Adjustment, &'static str> {
    match &key {
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
            if range.edges() != standard {
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
