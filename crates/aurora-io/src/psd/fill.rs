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
//! # Bounded work
//!
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

use aurora_core::Rect;
use half::f16;

use super::descriptor::{self, Descriptor, ItemBudget, Value};
use super::vector;

/// The longest `SoCo`/`GdFl` block read.
pub(super) const MAX_FILL_BLOCK_BYTES: usize = 1 << 20;

/// The most colour (and transparency) stops a gradient may hold.
pub(super) const MAX_STOPS: usize = 256;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Style {
    Linear,
    Radial,
    Angle,
    Reflected,
    Diamond,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Stop<T> {
    /// `0..=1`.
    pub pos: f32,
    /// The midpoint towards the next stop, `0..=1` (`0.5` = even).
    pub mid: f32,
    pub value: T,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Gradient {
    pub colours: Vec<Stop<[f32; 3]>>,
    pub alphas: Vec<Stop<f32>>,
    pub style: Style,
    pub angle: f64,
    pub scale: f64,
    pub reverse: bool,
    pub align: bool,
    /// Percent of the box width and height.
    pub offset: (f64, f64),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Fill {
    Solid([f32; 3]),
    Gradient(Gradient),
}

/// Parses a `SoCo` or `GdFl` block (`PtFl` is [`LeftOut::Pattern`]).
pub(super) fn parse(key: [u8; 4], data: &[u8]) -> Result<Fill, LeftOut> {
    if &key == b"PtFl" {
        return Err(LeftOut::Pattern);
    }
    if data.len() > MAX_FILL_BLOCK_BYTES {
        return Err(LeftOut::TooLarge);
    }
    let mut budget = ItemBudget::new();
    let d = descriptor::parse_adjustment_block(data, &mut budget)
        .map_err(|_| LeftOut::Unreadable)?;
    match &key {
        b"SoCo" => {
            let c = d.get(b"Clr ").and_then(Value::descriptor);
            Ok(Fill::Solid(colour(c.ok_or(LeftOut::Unreadable)?)?))
        }
        b"GdFl" => gradient(&d).map(Fill::Gradient),
        _ => Err(LeftOut::Unreadable),
    }
}

/// An `RGBC` (0..255) or grey (`Grsc`, ink percent) colour descriptor.
fn colour(c: &Descriptor) -> Result<[f32; 3], LeftOut> {
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
        let ink = c.get(b"Gry ").and_then(Value::number).filter(|v| v.is_finite());
        let g = (1.0 - ink.ok_or(LeftOut::Unreadable)? / 100.0).clamp(0.0, 1.0) as f32;
        Ok([g, g, g])
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

fn gradient(d: &Descriptor) -> Result<Gradient, LeftOut> {
    let grad = d
        .get(b"Grad")
        .and_then(Value::descriptor)
        .ok_or(LeftOut::Unreadable)?;
    if grad.get(b"GrdF").and_then(Value::enum_value) != Some(b"CstS") {
        return Err(LeftOut::Noise);
    }
    let colours = stops(
        grad.get(b"Clrs").ok_or(LeftOut::Unreadable)?,
        |s| colour(s.get(b"Clr ").and_then(Value::descriptor).ok_or(LeftOut::Unreadable)?),
    )?;
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

/// The value of a stop ramp at `t`: clamped outside the stops, linear
/// between them, bent by the earlier stop's midpoint.
fn ramp<T: Copy>(stops: &[Stop<T>], t: f32, lerp: impl Fn(T, T, f32) -> T) -> Option<T> {
    let first = stops.first()?;
    let last = stops.last()?;
    if t <= first.pos {
        return Some(first.value);
    }
    if t >= last.pos {
        return Some(last.value);
    }
    for pair in stops.windows(2) {
        let (a, b) = (pair.first()?, pair.get(1)?);
        if t >= a.pos && t < b.pos {
            let span = b.pos - a.pos;
            let mut u = if span > 0.0 { (t - a.pos) / span } else { 1.0 };
            let m = a.mid.clamp(0.001, 0.999);
            if (m - 0.5).abs() > 1e-6 {
                u = u.powf(0.5_f32.ln() / m.ln());
            }
            return Some(lerp(a.value, b.value, u));
        }
    }
    Some(last.value)
}

impl Gradient {
    /// The colour and opacity at `t` in `0..=1` (reverse already applied).
    fn at(&self, t: f32) -> ([f32; 3], f32) {
        let lerp3 = |a: [f32; 3], b: [f32; 3], u: f32| {
            [
                a[0] + (b[0] - a[0]) * u,
                a[1] + (b[1] - a[1]) * u,
                a[2] + (b[2] - a[2]) * u,
            ]
        };
        let rgb = ramp(&self.colours, t, lerp3).unwrap_or([0.0; 3]);
        let alpha = ramp(&self.alphas, t, |a, b, u| a + (b - a) * u).unwrap_or(1.0);
        (rgb, alpha)
    }

    /// The position `0..=1` along the gradient of document point `(x, y)`
    /// for a reference box `b`.
    pub(super) fn position(&self, x: f64, y: f64, b: Rect) -> f32 {
        let (w, h) = (f64::from(b.width), f64::from(b.height));
        let cx = b.x as f64 + w / 2.0 + self.offset.0 / 100.0 * w;
        let cy = b.y as f64 + h / 2.0 + self.offset.1 / 100.0 * h;
        let (dx, dy) = (x - cx, y - cy);
        let a = self.angle.to_radians();
        let (cos, sin) = (a.cos(), a.sin());
        let along = dx * cos - dy * sin;
        let across = dx * sin + dy * cos;
        let length = (self.scale * (w * cos.abs() + h * sin.abs())).max(1e-9);
        let t = match self.style {
            Style::Linear => 0.5 + along / length,
            Style::Reflected => along.abs() / (length / 2.0),
            Style::Radial => dx.hypot(dy) / (self.scale * w.hypot(h) / 2.0).max(1e-9),
            Style::Diamond => {
                (along.abs() + across.abs()) / (self.scale * (w + h) / 2.0).max(1e-9)
            }
            Style::Angle => {
                (dy.atan2(dx).to_degrees() + self.angle).rem_euclid(360.0) / 360.0
            }
        };
        let t = t.clamp(0.0, 1.0) as f32;
        if self.reverse { 1.0 - t } else { t }
    }
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
/// without one). `None` when the buffer cannot be allocated.
pub(super) fn render(
    fill: &Fill,
    region: Rect,
    reference: Rect,
    raster: Option<&vector::Raster>,
) -> Option<Vec<f16>> {
    let (w, h) = (region.width as usize, region.height as usize);
    let len = w.checked_mul(h)?.checked_mul(4)?;
    let mut out: Vec<f16> = Vec::new();
    out.try_reserve_exact(len).ok()?;
    for row in 0..h {
        let y = region.y + row as i64;
        for col in 0..w {
            let x = region.x + col as i64;
            let cover = coverage_at(raster, x, y);
            let (rgb, alpha) = match fill {
                Fill::Solid(c) => (*c, 1.0),
                Fill::Gradient(g) => g.at(g.position(x as f64, y as f64, reference)),
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
    let inside = x >= b.x
        && y >= b.y
        && x < b.x + i64::from(b.width)
        && y < b.y + i64::from(b.height);
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
}

/// Reads an artboard block. `Ok(None)` for a transparent background
/// (type `3`, nothing to draw). Type `4` is a custom colour
/// (`artboard-bgcolor.psd`: Photoshop's merged image shows exactly the
/// `Clr ` colour inside `artboardRect`); the other types are not drawn.
pub(super) fn parse_artboard(data: &[u8]) -> Result<Option<ArtboardBackground>, ArtboardLeft> {
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
    #[allow(clippy::float_cmp)] // a small stored integer
    match kind {
        k if k == 3.0 => return Ok(None),
        k if k == 4.0 => {}
        _ => return Err(ArtboardLeft::Unchecked),
    }
    let c = d
        .get(b"Clr ")
        .and_then(Value::descriptor)
        .ok_or(ArtboardLeft::Unreadable)?;
    let colour = colour(c).map_err(|_| ArtboardLeft::Unreadable)?;
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
    let (top, left, bottom, right) = (edge(b"Top ")?, edge(b"Left")?, edge(b"Btom")?, edge(b"Rght")?);
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

#[cfg(test)]
impl Gradient {
    /// [`Self::at`] for the tests.
    pub(super) fn at_for_test(&self, t: f32) -> ([f32; 3], f32) {
        self.at(t)
    }
}
