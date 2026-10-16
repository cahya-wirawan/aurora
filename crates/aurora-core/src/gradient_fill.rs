//! A Photoshop gradient fill: colour stops, transparency stops, a style
//! and its geometry (task 110, moved down from `aurora-io`'s `psd/fill.rs`
//! so the compositor's Gradient Overlay and the PSD gradient fill layers
//! share one evaluation).
//!
//! Pure data and arithmetic: nothing here reads a file or touches pixels.
//! The geometry is documented on [`GradientFill::frame`]; what was and was
//! not checked against Photoshop is in `aurora-io`'s `psd/fill.rs` and
//! PLAN.md.

use crate::Rect;

/// The most colour (and transparency) stops a gradient may hold.
pub const MAX_GRADIENT_STOPS: usize = 256;

/// The gradient shapes Photoshop offers for a fill or overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GradientStyle {
    Linear,
    Radial,
    Angle,
    Reflected,
    Diamond,
}

/// One stop of a ramp.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FillStop<T> {
    /// `0..=1`.
    pub pos: f32,
    /// The midpoint towards the next stop, `0..=1` (`0.5` = even).
    pub mid: f32,
    pub value: T,
}

/// A gradient: straight sRGB-encoded colours (`0..=1`), opacities
/// (`0..=1`) and the geometry that places them.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GradientFill {
    pub colours: Vec<FillStop<[f32; 3]>>,
    pub alphas: Vec<FillStop<f32>>,
    pub style: GradientStyle,
    /// Degrees.
    pub angle: f64,
    /// `0 < scale <= 100` (the file's percent / 100).
    pub scale: f64,
    pub reverse: bool,
    /// "Align with layer": the reference box is the layer's, not the
    /// document's.
    pub align: bool,
    /// Percent of the box width and height.
    pub offset: (f64, f64),
}

/// A gradient fill refused by [`GradientFill::validate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("gradient setting {field} is not a finite number within its range")]
pub struct GradientError {
    pub field: &'static str,
}

#[allow(clippy::many_single_char_names)] // ramp maths, as in `aurora-io`'s old copy
fn ramp<T: Copy>(stops: &[FillStop<T>], t: f32, lerp: impl Fn(T, T, f32) -> T) -> Option<T> {
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

fn unit_f32(v: f32, field: &'static str) -> Result<(), GradientError> {
    if v.is_finite() && (0.0..=1.0).contains(&v) {
        Ok(())
    } else {
        Err(GradientError { field })
    }
}

impl GradientFill {
    /// Checks the stop counts (`1..=`[`MAX_GRADIENT_STOPS`] colours, at
    /// most that many opacities), every number finite and in range, stops
    /// in ascending order, `0 < scale <= 100`, `|offset| <= 1000` percent.
    ///
    /// # Errors
    ///
    /// The first [`GradientError`] found.
    pub fn validate(&self) -> Result<(), GradientError> {
        if self.colours.is_empty() || self.colours.len() > MAX_GRADIENT_STOPS {
            return Err(GradientError {
                field: "colour stops",
            });
        }
        if self.alphas.len() > MAX_GRADIENT_STOPS {
            return Err(GradientError {
                field: "opacity stops",
            });
        }
        let mut last = 0.0_f32;
        for s in &self.colours {
            unit_f32(s.pos, "stop position")?;
            unit_f32(s.mid, "midpoint")?;
            if s.pos < last {
                return Err(GradientError {
                    field: "stop order",
                });
            }
            last = s.pos;
            for c in s.value {
                unit_f32(c, "stop colour")?;
            }
        }
        last = 0.0;
        for s in &self.alphas {
            unit_f32(s.pos, "stop position")?;
            unit_f32(s.mid, "midpoint")?;
            if s.pos < last {
                return Err(GradientError {
                    field: "stop order",
                });
            }
            last = s.pos;
            unit_f32(s.value, "stop opacity")?;
        }
        if !(self.angle.is_finite() && self.angle.abs() <= 720.0) {
            return Err(GradientError { field: "angle" });
        }
        if !(self.scale.is_finite() && self.scale > 0.0 && self.scale <= 100.0) {
            return Err(GradientError { field: "scale" });
        }
        let (ox, oy) = self.offset;
        if !(ox.is_finite() && oy.is_finite() && ox.abs() <= 1000.0 && oy.abs() <= 1000.0) {
            return Err(GradientError { field: "offset" });
        }
        Ok(())
    }

    /// The colour and opacity at `t` in `0..=1` (reverse already applied).
    #[must_use]
    pub fn at(&self, t: f32) -> ([f32; 3], f32) {
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

    /// The geometry for reference box `b`, with the trigonometry done
    /// once. With `d = (cos a, -sin a)` (image rows grow downwards), `c`
    /// the box centre moved by the offset percentages and
    /// `L = scale * (w |cos a| + h |sin a|)`:
    ///
    /// - Linear: `t = 0.5 + (p - c) . d / L`;
    /// - Reflected: `t = |(p - c) . d| / (L / 2)`;
    /// - Radial: `t = |p - c| / (scale * hypot(w, h) / 2)`;
    /// - Diamond: `t = (|u| + |v|) / (scale * (w + h) / 2)`;
    /// - Angle: `t = ((deg(atan2(y, x)) + a) mod 360) / 360`.
    #[must_use]
    pub fn frame(&self, b: Rect) -> GradientFrame<'_> {
        let (w, h) = (f64::from(b.width), f64::from(b.height));
        let a = self.angle.to_radians();
        let (cos, sin) = (a.cos(), a.sin());
        #[allow(clippy::cast_precision_loss)] // document coordinates are small
        let (bx, by) = (b.x as f64, b.y as f64);
        GradientFrame {
            gradient: self,
            cx: bx + w / 2.0 + self.offset.0 / 100.0 * w,
            cy: by + h / 2.0 + self.offset.1 / 100.0 * h,
            cos,
            sin,
            length: (self.scale * (w * cos.abs() + h * sin.abs())).max(1e-9),
            radius: (self.scale * w.hypot(h) / 2.0).max(1e-9),
            diamond: (self.scale * (w + h) / 2.0).max(1e-9),
        }
    }

    /// The position `0..=1` along the gradient of document point `(x, y)`
    /// for a reference box `b` (one-shot form of [`GradientFrame`]).
    #[must_use]
    pub fn position(&self, x: f64, y: f64, b: Rect) -> f32 {
        self.frame(b).position(x, y)
    }
}

/// A gradient's per-box constants (centre, direction, lengths).
#[derive(Debug)]
pub struct GradientFrame<'a> {
    gradient: &'a GradientFill,
    cx: f64,
    cy: f64,
    cos: f64,
    sin: f64,
    length: f64,
    radius: f64,
    diamond: f64,
}

impl GradientFrame<'_> {
    /// The position `0..=1` along the gradient of `(x, y)`, reverse
    /// applied.
    #[must_use]
    pub fn position(&self, x: f64, y: f64) -> f32 {
        let g = self.gradient;
        let (dx, dy) = (x - self.cx, y - self.cy);
        let along = dx * self.cos - dy * self.sin;
        let across = dx * self.sin + dy * self.cos;
        let t = match g.style {
            GradientStyle::Linear => 0.5 + along / self.length,
            GradientStyle::Reflected => along.abs() / (self.length / 2.0),
            GradientStyle::Radial => dx.hypot(dy) / self.radius,
            GradientStyle::Diamond => (along.abs() + across.abs()) / self.diamond,
            GradientStyle::Angle => (dy.atan2(dx).to_degrees() + g.angle).rem_euclid(360.0) / 360.0,
        };
        #[allow(clippy::cast_possible_truncation)] // clamped to 0..=1
        let t = t.clamp(0.0, 1.0) as f32;
        if g.reverse { 1.0 - t } else { t }
    }
}
