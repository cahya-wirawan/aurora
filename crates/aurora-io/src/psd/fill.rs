//! Solid-colour and gradient fill layers that store no pixels (task 108).
//!
//! Photoshop saves a fill layer (`SoCo` solid colour, `GdFl` gradient) with
//! an empty rectangle and no channel data whenever its picture can be
//! regenerated from the fill settings and the layer's vector mask. Aurora
//! has no fill-layer model, so such a layer used to vanish from the opened
//! document (`Note::FillSkipped`). It is now drawn at open time: the fill
//! (a colour, or a gradient evaluated per pixel) times the vector mask's
//! anti-aliased coverage becomes the layer's straight-alpha pixels, an
//! ordinary pixel layer (a PSD save writes those pixels; the fill settings
//! are not kept).
//!
//! # Memory
//!
//! A drawn fill is built eagerly: one `Vec<f16>` of the whole region at
//! 8 bytes per pixel (straight RGBA), plus the vector mask's coverage
//! plane at 2 bytes per pixel while it is drawn. A full-canvas fill (no
//! vector mask, an inverted one, a mask density below 100 %) therefore
//! costs 8 bytes per canvas pixel, and the shape's rasterisation 10 more
//! at its peak. Both are charged to the file's vector pixel budget
//! (`vector::Budget::pixels`) before any buffer exists, so a hostile or
//! very large canvas leaves the fill out with a report line instead of
//! allocating.
//!
//! # Bounded work
//!//!
//! The block is read through the descriptor parser (depth and item
//! budgets), is at most [`MAX_FILL_BLOCK_BYTES`], holds at most
//! [`MAX_STOPS`] stops, and the region it is drawn over is charged to the
//! file's vector pixel budget before any buffer exists.
//!
//! # Gradient geometry, and what was checked
//!
//! The reference box is the vector path's bounding box (or the canvas when
//! there is no path or "Align with layer" is off). With `d = (cos a, -sin a)`
//! (image rows grow downwards), `c` the box centre moved by the offset
//! percentages and `L = scale * (w |cos a| + h |sin a|)`:
//!
//! - Linear: `t = 0.5 + (p - c) . d / L`;
//! - Reflected: `t = |(p - c) . d| / (L / 2)`;
//! - Radial: `t = |p - c| / (scale * hypot(w, h) / 2)`;
//! - Diamond: `t = (|u| + |v|) / (scale * (w + h) / 2)` with `u`, `v` the
//!   coordinates along `d` and across it;
//! - Angle: `t = ((deg(atan2(y, x)) + a) mod 360) / 360` (psd-tools'
//!   reading).
//!
//! Only the Linear form has a Photoshop picture to be compared with
//! (`vector-mask2.psd`: it matches in direction, orientation and sampling
//! at the integer pixel position, but Photoshop's gradient is about 16 %
//! shorter than `L`, unexplained) and the Reflected form has one tiny
//! picture (`4x4_16bit_rgb.psd`) whose symmetry axis does not match the
//! stored 30 degrees. The other forms are unchecked.

// Pixel/geometry arithmetic on small, bounded values, as in `vector.rs`.
#![allow(
    clippy::many_single_char_names,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use aurora_core::Rect;
use half::f16;

use super::descriptor::{self, Descriptor, ItemBudget, Value};
use super::vector;

/// The longest `SoCo`/`GdFl` block read.
pub(super) const MAX_FILL_BLOCK_BYTES: usize = 1 << 20;

/// The most colour (and transparency) stops a gradient may hold.
pub(super) const MAX_STOPS: usize = aurora_core::MAX_GRADIENT_STOPS;

/// Why a fill is not drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum LeftOut {
    /// A pattern fill: the pattern's pixels are not read.
    Pattern,
    /// The block does not read.
    Unreadable,
    /// A colour that is not RGB or grey.
    Colour,
    /// A Noise gradient.
    Noise,
    /// A gradient style Aurora does not know.
    Style,
    /// Over a work or memory bound.
    TooLarge,
}

pub(super) use aurora_core::gradient_fill::{
    FillStop as Stop, GradientFill as Gradient, GradientStyle as Style,
};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Fill {
    Solid([f32; 3]),
    Gradient(Gradient),
}

/// How a CMYK file's `CMYC` colours become sRGB (task 146); `None` for an
/// RGB or Grayscale file, where a `CMYC` colour is left out.
pub(super) type CmykColours<'a> = Option<&'a crate::colour_cmyk::CmykConverter>;

/// Parses a `SoCo` or `GdFl` block (`PtFl` is [`LeftOut::Pattern`]).
#[cfg(test)]
pub(super) fn parse(key: [u8; 4], data: &[u8]) -> Result<Fill, LeftOut> {
    parse_in(key, data, None)
}

/// [`parse`] for a file whose colours may be CMYK.
pub(super) fn parse_in(key: [u8; 4], data: &[u8], cmyk: CmykColours<'_>) -> Result<Fill, LeftOut> {
    if &key == b"PtFl" {
        return Err(LeftOut::Pattern);
    }
    if data.len() > MAX_FILL_BLOCK_BYTES {
        return Err(LeftOut::TooLarge);
    }
    let mut budget = ItemBudget::new();
    let d =
        descriptor::parse_adjustment_block(data, &mut budget).map_err(|_| LeftOut::Unreadable)?;
    match &key {
        b"SoCo" => {
            let c = d.get(b"Clr ").and_then(Value::descriptor);
            Ok(Fill::Solid(colour_in(c.ok_or(LeftOut::Unreadable)?, cmyk)?))
        }
        b"GdFl" => gradient_in(&d, cmyk).map(Fill::Gradient),
        _ => Err(LeftOut::Unreadable),
    }
}

/// An `RGBC` (0..255), grey (`Grsc`, ink percent) or, in a CMYK file,
/// `CMYC` (four ink percentages) colour descriptor.
fn colour_in(c: &Descriptor, cmyk: CmykColours<'_>) -> Result<[f32; 3], LeftOut> {
    let channel = |key: &[u8]| -> Option<f32> {
        let v = c.get(key)?.number()?;
        v.is_finite().then(|| (v / 255.0).clamp(0.0, 1.0) as f32)
    };
    if c.get(b"Rd  ").is_some() {
        match (channel(b"Rd  "), channel(b"Grn "), channel(b"Bl  ")) {
            (Some(r), Some(g), Some(b)) => Ok([r, g, b]),
            _ => Err(LeftOut::Unreadable),
        }
    } else if c.get(b"Gry ").is_some() {
        // A Grayscale colour is the ink percentage: 100 is black (checked
        // on `4x4_16bit_grayscale.psd`: 90.6 shows as level 24).
        let ink = c
            .get(b"Gry ")
            .and_then(Value::number)
            .filter(|v| v.is_finite());
        let g = (1.0 - ink.ok_or(LeftOut::Unreadable)? / 100.0).clamp(0.0, 1.0) as f32;
        Ok([g, g, g])
    } else if c.get(b"Cyn ").is_some() {
        // Ink percentages, not inverted (task 146).
        let ink = |key: &[u8]| -> Option<f32> {
            let v = c.get(key)?.number()?;
            v.is_finite().then(|| (v / 100.0).clamp(0.0, 1.0) as f32)
        };
        match (ink(b"Cyn "), ink(b"Mgnt"), ink(b"Ylw "), ink(b"Blck"), cmyk) {
            (Some(c), Some(m), Some(y), Some(k), Some(conv)) => Ok(conv.colour([c, m, y, k])),
            (Some(_), Some(_), Some(_), Some(_), None) => Err(LeftOut::Colour),
            _ => Err(LeftOut::Unreadable),
        }
    } else {
        Err(LeftOut::Colour)
    }
}

fn num(d: &Descriptor, key: &[u8], default: f64) -> Result<f64, LeftOut> {
    match d.get(key) {
        None => Ok(default),
        Some(v) => v
            .number()
            .filter(|n| n.is_finite())
            .ok_or(LeftOut::Unreadable),
    }
}

fn flag(d: &Descriptor, key: &[u8], default: bool) -> Result<bool, LeftOut> {
    d.get(key)
        .map_or(Ok(default), |v| v.bool().ok_or(LeftOut::Unreadable))
}

fn stops<T>(
    list: &Value,
    value: impl Fn(&Descriptor) -> Result<T, LeftOut>,
) -> Result<Vec<Stop<T>>, LeftOut> {
    let items = list.list().ok_or(LeftOut::Unreadable)?;
    if items.len() > MAX_STOPS {
        return Err(LeftOut::TooLarge);
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let d = item.descriptor().ok_or(LeftOut::Unreadable)?;
        let pos = (num(d, b"Lctn", 0.0)? / 4096.0).clamp(0.0, 1.0) as f32;
        let mid = (num(d, b"Mdpn", 50.0)? / 100.0).clamp(0.0, 1.0) as f32;
        out.push(Stop {
            pos,
            mid,
            value: value(d)?,
        });
    }
    out.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    Ok(out)
}

pub(super) fn gradient(d: &Descriptor) -> Result<Gradient, LeftOut> {
    gradient_in(d, None)
}

pub(super) fn gradient_in(d: &Descriptor, cmyk: CmykColours<'_>) -> Result<Gradient, LeftOut> {
    let grad = d
        .get(b"Grad")
        .and_then(Value::descriptor)
        .ok_or(LeftOut::Unreadable)?;
    if grad.get(b"GrdF").and_then(Value::enum_value) != Some(b"CstS") {
        return Err(LeftOut::Noise);
    }
    let colours = stops(grad.get(b"Clrs").ok_or(LeftOut::Unreadable)?, |s| {
        colour_in(
            s.get(b"Clr ")
                .and_then(Value::descriptor)
                .ok_or(LeftOut::Unreadable)?,
            cmyk,
        )
    })?;
    if colours.is_empty() {
        return Err(LeftOut::Unreadable);
    }
    let alphas = match grad.get(b"Trns") {
        None => Vec::new(),
        Some(list) => stops(list, |s| {
            Ok((num(s, b"Opct", 100.0)? / 100.0).clamp(0.0, 1.0) as f32)
        })?,
    };
    let style = match d.get(b"Type").and_then(Value::enum_value) {
        None | Some(b"Lnr ") => Style::Linear,
        Some(b"Rdl ") => Style::Radial,
        Some(b"Angl") => Style::Angle,
        Some(b"Rflc") => Style::Reflected,
        Some(b"Dmnd") => Style::Diamond,
        Some(_) => return Err(LeftOut::Style),
    };
    let offset = match d.get(b"Ofst").and_then(Value::descriptor) {
        None => (0.0, 0.0),
        Some(o) => (num(o, b"Hrzn", 0.0)?, num(o, b"Vrtc", 0.0)?),
    };
    let scale = num(d, b"Scl ", 100.0)? / 100.0;
    if !(scale > 0.0 && scale <= 100.0) {
        return Err(LeftOut::Unreadable);
    }
    Ok(Gradient {
        colours,
        alphas,
        style,
        angle: num(d, b"Angl", 0.0)?,
        scale,
        reverse: flag(d, b"Rvrs", false)?,
        align: flag(d, b"Algn", true)?,
        offset,
    })
}

/// The reference box of a gradient: the path's bounds, else the canvas.
pub(super) fn reference_box(g: &Gradient, path: Option<Rect>, canvas: Rect) -> Rect {
    match path {
        Some(p) if g.align && p.width > 0 && p.height > 0 => p,
        _ => canvas,
    }
}

/// The straight-alpha `f16` RGBA pixels of `fill` over `region`, its
/// opacity multiplied by the vector `raster`'s coverage (everywhere `1`
/// without one), the coverage first raised by the vector mask's `density`
/// (`d * c + (1 - d)`, the rule `aurora-doc` applies to a mask density).
/// `None` when the buffer cannot be allocated.
pub(super) fn render(
    fill: &Fill,
    region: Rect,
    reference: Rect,
    raster: Option<&vector::Raster>,
    density: f32,
) -> Option<Vec<f16>> {
    let (w, h) = (region.width as usize, region.height as usize);
    let len = w.checked_mul(h)?.checked_mul(4)?;
    let mut out: Vec<f16> = Vec::new();
    out.try_reserve_exact(len).ok()?;
    let frame = match fill {
        Fill::Gradient(g) => Some(g.frame(reference)),
        Fill::Solid(_) => None,
    };
    for row in 0..h {
        let y = region.y + row as i64;
        for col in 0..w {
            let x = region.x + col as i64;
            let cover = coverage_at(raster, x, y);
            let cover = if raster.is_some() {
                density * cover + (1.0 - density)
            } else {
                cover
            };
            let (rgb, alpha) = match (fill, &frame) {
                (Fill::Gradient(g), Some(f)) => g.at(f.position(x as f64, y as f64)),
                (Fill::Solid(c), _) => (*c, 1.0),
                (Fill::Gradient(_), None) => ([0.0; 3], 1.0),
            };
            out.push(f16::from_f32(rgb[0]));
            out.push(f16::from_f32(rgb[1]));
            out.push(f16::from_f32(rgb[2]));
            out.push(f16::from_f32((alpha * cover).clamp(0.0, 1.0)));
        }
    }
    Some(out)
}

fn coverage_at(raster: Option<&vector::Raster>, x: i64, y: i64) -> f32 {
    let Some(r) = raster else { return 1.0 };
    let b = r.bounds;
    let inside =
        x >= b.x && y >= b.y && x < b.x + i64::from(b.width) && y < b.y + i64::from(b.height);
    if !inside {
        return if r.outside { 1.0 } else { 0.0 };
    }
    let index = (y - b.y) as usize * b.width as usize + (x - b.x) as usize;
    r.coverage.get(index).map_or(0.0, |c| c.to_f32())
}

// ---------------------------------------------------------------------
// Artboard background colour (task 108)
// ---------------------------------------------------------------------

/// The most an `artb`/`artd`/`abdd` block may hold.
pub(super) const MAX_ARTBOARD_BLOCK_BYTES: usize = 1 << 16;

/// An artboard's custom background colour over its rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ArtboardBackground {
    pub rect: Rect,
    pub colour: [f32; 3],
}

/// Why an artboard's background is not drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ArtboardLeft {
    /// White or black (types `1` and `2`): no corpus picture shows them.
    Unchecked,
    Unreadable,
    /// The rectangle is empty or lies entirely off the canvas.
    Empty,
    /// Over the pixel budget, or the buffer cannot be allocated.
    TooLarge,
}

/// Reads an artboard block. `Ok(None)` for a transparent background
/// (type `3`, nothing to draw). Type `4` is a custom colour
/// (`artboard-bgcolor.psd`: Photoshop's merged image shows exactly the
/// `Clr ` colour inside `artboardRect`); the other types are not drawn.
#[cfg(test)]
pub(super) fn parse_artboard(data: &[u8]) -> Result<Option<ArtboardBackground>, ArtboardLeft> {
    parse_artboard_in(data, None)
}

/// [`parse_artboard`] for a file whose colours may be CMYK.
pub(super) fn parse_artboard_in(
    data: &[u8],
    cmyk: CmykColours<'_>,
) -> Result<Option<ArtboardBackground>, ArtboardLeft> {
    if data.len() > MAX_ARTBOARD_BLOCK_BYTES {
        return Err(ArtboardLeft::Unreadable);
    }
    let mut budget = ItemBudget::new();
    let d = descriptor::parse_adjustment_block(data, &mut budget)
        .map_err(|_| ArtboardLeft::Unreadable)?;
    let kind = d
        .get(b"artboardBackgroundType")
        .and_then(Value::number)
        .ok_or(ArtboardLeft::Unreadable)?;
    // A small stored integer: 3 is transparent, 4 a custom colour.
    if (kind - 3.0).abs() < 0.5 {
        return Ok(None);
    }
    if (kind - 4.0).abs() >= 0.5 {
        return Err(ArtboardLeft::Unchecked);
    }
    let c = d
        .get(b"Clr ")
        .and_then(Value::descriptor)
        .ok_or(ArtboardLeft::Unreadable)?;
    let colour = colour_in(c, cmyk).map_err(|_| ArtboardLeft::Unreadable)?;
    let r = d
        .get(b"artboardRect")
        .and_then(Value::descriptor)
        .ok_or(ArtboardLeft::Unreadable)?;
    let edge = |key: &[u8]| -> Result<i64, ArtboardLeft> {
        let v = r
            .get(key)
            .and_then(Value::number)
            .filter(|v| v.is_finite() && v.abs() < 1e9)
            .ok_or(ArtboardLeft::Unreadable)?;
        Ok(v.round() as i64)
    };
    let (top, left, bottom, right) = (
        edge(b"Top ")?,
        edge(b"Left")?,
        edge(b"Btom")?,
        edge(b"Rght")?,
    );
    let width = right.saturating_sub(left).max(0);
    let height = bottom.saturating_sub(top).max(0);
    Ok(Some(ArtboardBackground {
        rect: Rect {
            x: left,
            y: top,
            width: u32::try_from(width).map_err(|_| ArtboardLeft::Unreadable)?,
            height: u32::try_from(height).map_err(|_| ArtboardLeft::Unreadable)?,
        },
        colour,
    }))
}
