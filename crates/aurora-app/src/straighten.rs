//! Straighten for the Crop tool (task 180): the pure geometry and the
//! resampler. The document operation is [`crate::straighten_doc`].
//!
//! **Everything here is a choice, not compared with Photoshop.**
//!
//! # The frame
//!
//! A straighten has an angle `theta` in degrees, `|theta| <= 45`, measured
//! the way the document's y-down coordinates make it: positive turns the
//! picture's horizontal clockwise on screen. The *straightened frame* `F` has
//! the same size as the canvas and the same centre `Cc = (W / 2, H / 2)`;
//! a point `f` of `F` is the source point
//!
//! ```text
//! S = Cc + R(theta) * (f - Cc),   R(theta) = [[cos, -sin], [sin, cos]]
//! ```
//!
//! so the frame's x axis runs along the source direction `(cos, sin)` and a
//! source line at angle `theta` is horizontal in `F`. The crop rectangle is
//! an axis-aligned whole-pixel rectangle `rect` in `F` coordinates (the
//! *output* coordinates), and the output pixel `(u, v)` of a committed
//! straighten has its centre at `f = (rect.x + u + 0.5, rect.y + v + 0.5)`,
//! i.e. at the source point
//!
//! ```text
//! C + R(theta) * ((u + 0.5 - w / 2), (v + 0.5 - h / 2)),   C = S(centre of rect)
//! ```
//!
//! which is the formula in the task description ([`Frame::output_to_source`]).
//!
//! # The valid area
//!
//! A rectangle is valid when its four rotated corners lie inside the canvas
//! shrunk by [`TAP_MARGIN`] pixels on every side (the bicubic resampler's
//! taps reach two pixels either way, so a layer as large as the canvas never
//! samples outside itself and a constant layer stays exactly constant). The
//! crop rectangle is kept valid by every gesture: a move or resize that would
//! leave the area stops where it still fits.
//!
//! # The line
//!
//! A drawn line gives the angle that makes it horizontal when it is within
//! 45 degrees of horizontal and vertical otherwise ([`angle_from_line`]).
//!
//! # The resampler
//!
//! Catmull-Rom bicubic ([`catmull_rom_weights`]) on premultiplied `f32`,
//! 4 x 4 taps, a tap outside the source window is transparent. Pixel results
//! are clamped to a valid premultiplied texel (alpha `0..=1`, each colour
//! `0..=alpha`) and stored as `f16`; mask and selection coverage is a single
//! plane clamped to `0..=1`.

#![allow(clippy::many_single_char_names, clippy::wrong_self_convention)]

use aurora_core::Rect;
use aurora_tile::{CHANNELS, TILE, TileId};
use half::f16;

use crate::crop::Ratio;

/// The largest straighten angle, in degrees.
pub(crate) const MAX_ANGLE_DEG: f64 = 45.0;

/// An angle smaller than this (degrees) is no angle at all.
pub(crate) const MIN_ANGLE_DEG: f64 = 0.001;

/// How far inside the canvas the rotated crop corners must stay, in pixels
/// (see the module docs).
pub(crate) const TAP_MARGIN: f64 = 2.0;

/// The shortest straighten line, in logical (screen) pixels: a shorter drag
/// changes nothing (an interaction tolerance, not a style value).
pub(crate) const MIN_LINE_LOGICAL_PX: f32 = 4.0;

/// The palette command that asks for an angle.
pub(crate) const COMMAND_ID: &str = "crop.straighten_by_angle";

/// The History row of a committed straighten.
pub(crate) const STRAIGHTEN_LABEL: &str = "Straighten";

/// The most output pixels one straighten resamples per surface (64 Mpx,
/// the mask cap's convention): a larger canvas is refused.
pub(crate) const MAX_SURFACE_PIXELS: u64 = 64_000_000;

/// The most pixels resampled in all (output pixels x surfaces): 1 Gpx.
pub(crate) const MAX_TOTAL_PIXELS: u64 = 1_000_000_000;

/// The angle, in degrees, a typed or drawn value means: `Ok(None)` for no
/// angle (|a| below [`MIN_ANGLE_DEG`]), `Err` for a value that is not finite
/// or beyond [`MAX_ANGLE_DEG`].
///
/// # Errors
///
/// A short sentence for the prompt's row.
pub(crate) fn checked_angle(degrees: f64) -> Result<Option<f64>, String> {
    if !degrees.is_finite() {
        return Err("That is not a number.".to_owned());
    }
    if degrees.abs() > MAX_ANGLE_DEG {
        return Err(format!(
            "The angle must be between -{MAX_ANGLE_DEG} and {MAX_ANGLE_DEG} degrees."
        ));
    }
    Ok((degrees.abs() >= MIN_ANGLE_DEG).then_some(degrees))
}

/// The angle a straighten line `a -> b` (document points) asks for, in
/// degrees: the rotation that makes the line horizontal when it is within
/// 45 degrees of horizontal (including exactly 45), else vertical. `None`
/// when a point is not finite or the line is shorter than `min_len`
/// document pixels.
pub(crate) fn angle_from_line(a: (f32, f32), b: (f32, f32), min_len: f32) -> Option<f64> {
    let (dx, dy) = (
        f64::from(b.0) - f64::from(a.0),
        f64::from(b.1) - f64::from(a.1),
    );
    if !(dx.is_finite() && dy.is_finite() && min_len.is_finite() && min_len >= 0.0) {
        return None;
    }
    let length = dx.hypot(dy);
    if length < f64::from(min_len) || length == 0.0 {
        return None;
    }
    // The line's direction as an undirected angle in (-90, 90].
    let mut phi = dy.atan2(dx).to_degrees();
    if phi > 90.0 {
        phi -= 180.0;
    } else if phi <= -90.0 {
        phi += 180.0;
    }
    if phi.abs() <= MAX_ANGLE_DEG {
        Some(phi)
    } else if phi > 0.0 {
        Some(phi - 90.0)
    } else {
        Some(phi + 90.0)
    }
}

/// The straightened frame of a canvas (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Frame {
    canvas: (u32, u32),
    /// `cos` and `sin` of the angle.
    cos: f64,
    sin: f64,
}

impl Frame {
    /// The frame of `canvas` turned by `degrees`; `None` for a canvas with
    /// no area or an angle that is not a usable one ([`checked_angle`]).
    pub(crate) fn new(canvas: (u32, u32), degrees: f64) -> Option<Self> {
        if canvas.0 == 0 || canvas.1 == 0 {
            return None;
        }
        let degrees = checked_angle(degrees).ok().flatten()?;
        let radians = degrees.to_radians();
        Some(Self {
            canvas,
            cos: radians.cos(),
            sin: radians.sin(),
        })
    }

    fn centre(&self) -> (f64, f64) {
        (
            f64::from(self.canvas.0) / 2.0,
            f64::from(self.canvas.1) / 2.0,
        )
    }

    /// The source point of the frame point `f`.
    pub(crate) fn to_source(&self, f: (f64, f64)) -> (f64, f64) {
        let (cx, cy) = self.centre();
        let (dx, dy) = (f.0 - cx, f.1 - cy);
        (
            cx + self.cos * dx - self.sin * dy,
            cy + self.sin * dx + self.cos * dy,
        )
    }

    /// The frame point of the source point `p` (the inverse of
    /// [`Self::to_source`]).
    pub(crate) fn from_source(&self, p: (f64, f64)) -> (f64, f64) {
        let (cx, cy) = self.centre();
        let (dx, dy) = (p.0 - cx, p.1 - cy);
        (
            cx + self.cos * dx + self.sin * dy,
            cy - self.sin * dx + self.cos * dy,
        )
    }

    /// The source point the centre of output pixel `(u, v)` of a straighten
    /// to `rect` samples: `C + R * ((u + 0.5 - w / 2), (v + 0.5 - h / 2))`
    /// with `C` the source point of the rectangle's centre. Equal to
    /// `to_source(rect.x + u + 0.5, rect.y + v + 0.5)`.
    pub(crate) fn output_to_source(&self, rect: &Rect, u: f64, v: f64) -> (f64, f64) {
        let half = (f64::from(rect.width) / 2.0, f64::from(rect.height) / 2.0);
        #[allow(clippy::cast_precision_loss)]
        let centre = self.to_source((rect.x as f64 + half.0, rect.y as f64 + half.1));
        let (dx, dy) = (u + 0.5 - half.0, v + 0.5 - half.1);
        (
            centre.0 + self.cos * dx - self.sin * dy,
            centre.1 + self.sin * dx + self.cos * dy,
        )
    }

    /// The affine map from source space to the output of a straighten to
    /// `rect`, as `[a, b, c, d, e, f]` for `(x, y) -> (a*x + c*y + e,
    /// b*x + d*y + f)` ([`aurora_doc::PathSet::transform`]): the inverse
    /// rotation about the canvas centre, then the shift by the rectangle's
    /// top-left corner.
    pub(crate) fn source_to_output_matrix(&self, rect: &Rect) -> [f64; 6] {
        let (cx, cy) = self.centre();
        let (c, s) = (self.cos, self.sin);
        #[allow(clippy::cast_precision_loss)]
        let (rx, ry) = (rect.x as f64, rect.y as f64);
        [
            c,
            -s,
            s,
            c,
            cx - (c * cx + s * cy) - rx,
            cy - (-s * cx + c * cy) - ry,
        ]
    }

    /// The four corners of `rect` (top-left, top-right, bottom-right,
    /// bottom-left) in source space.
    pub(crate) fn corners(&self, rect: &Rect) -> [(f64, f64); 4] {
        #[allow(clippy::cast_precision_loss)]
        let (l, t, r, b) = (
            rect.x as f64,
            rect.y as f64,
            rect.right() as f64,
            rect.bottom() as f64,
        );
        [(l, t), (r, t), (r, b), (l, b)].map(|f| self.to_source(f))
    }

    /// The eight handle points of `rect` (corners and edge midpoints, in the
    /// order of [`crate::crop::CropHandle::ALL`]) in source space.
    pub(crate) fn handle_points(&self, rect: &Rect) -> [(f64, f64); 8] {
        #[allow(clippy::cast_precision_loss)]
        let (l, t, r, b) = (
            rect.x as f64,
            rect.y as f64,
            rect.right() as f64,
            rect.bottom() as f64,
        );
        let (cx, cy) = (f64::midpoint(l, r), f64::midpoint(t, b));
        [
            (l, t),
            (cx, t),
            (r, t),
            (r, cy),
            (r, b),
            (cx, b),
            (l, b),
            (l, cy),
        ]
        .map(|f| self.to_source(f))
    }

    /// Whether all four rotated corners of `rect` lie inside the canvas
    /// shrunk by [`TAP_MARGIN`] (a convex area, so the whole rectangle does).
    pub(crate) fn rect_is_valid(&self, rect: &Rect) -> bool {
        if rect.is_empty() {
            return false;
        }
        let (w, h) = (f64::from(self.canvas.0), f64::from(self.canvas.1));
        let eps = 1.0e-6;
        self.corners(rect).iter().all(|&(x, y)| {
            x >= TAP_MARGIN - eps
                && x <= w - TAP_MARGIN + eps
                && y >= TAP_MARGIN - eps
                && y <= h - TAP_MARGIN + eps
        })
    }

    /// The largest valid rectangle centred on the canvas centre (to within
    /// half a pixel), of the aspect `ratio` when given. `None` when the
    /// canvas is too small for any.
    pub(crate) fn largest_rect(&self, ratio: Option<Ratio>) -> Option<Rect> {
        let x_room = f64::from(self.canvas.0) / 2.0 - TAP_MARGIN;
        let y_room = f64::from(self.canvas.1) / 2.0 - TAP_MARGIN;
        if x_room <= 0.0 || y_room <= 0.0 {
            return None;
        }
        let (c, s) = (self.cos.abs(), self.sin.abs());
        // Real-valued width and height; `fit` then makes them whole pixels.
        let (width, height) = match ratio {
            None => {
                let (a, b) = best_half_extents(c, s, x_room, y_room);
                (2.0 * a, 2.0 * b)
            }
            Some(ratio) => {
                let (rw, rh) = (f64::from(ratio.w), f64::from(ratio.h));
                let k = f64::min(
                    2.0 * x_room / (c * rw + s * rh),
                    2.0 * y_room / (s * rw + c * rh),
                );
                (k * rw, k * rh)
            }
        };
        self.fit(width, height, ratio)
    }

    /// The whole-pixel rectangle centred on the canvas at about `width` x
    /// `height`, shrunk until it is valid.
    fn fit(&self, width: f64, height: f64, ratio: Option<Ratio>) -> Option<Rect> {
        let centred = |w: u64, h: u64| -> Option<Rect> {
            let (w32, h32) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
            if w32 == 0 || h32 == 0 {
                return None;
            }
            let x = (i64::from(self.canvas.0) - i64::from(w32)).div_euclid(2);
            let y = (i64::from(self.canvas.1) - i64::from(h32)).div_euclid(2);
            Some(Rect {
                x,
                y,
                width: w32,
                height: h32,
            })
        };
        // Both are finite, positive and far below 2^53.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (mut w, mut h) = (
            (width + 1.0e-9).floor().max(1.0) as u64,
            (height + 1.0e-9).floor().max(1.0) as u64,
        );
        if let Some(ratio) = ratio {
            h = ratio.height_for(w);
        } else {
            // The real optimum is rarely whole: look at the pixels around it
            // for the largest valid area.
            let best = (0..8_u64)
                .flat_map(|dw| (0..8_u64).map(move |dh| (dw, dh)))
                .filter_map(|(dw, dh)| centred((w + 1).checked_sub(dw)?, (h + 1).checked_sub(dh)?))
                .filter(|r| self.rect_is_valid(r))
                .max_by_key(Rect::area_px);
            if best.is_some() {
                return best;
            }
        }
        for tries in 0..4096_u32 {
            if let Some(rect) = centred(w, h)
                && self.rect_is_valid(&rect)
            {
                return Some(rect);
            }
            if w <= 1 && h <= 1 {
                return None;
            }
            if let Some(ratio) = ratio {
                w = if tries < 16 {
                    w.saturating_sub(1)
                } else {
                    w.saturating_sub((w / 16).max(1))
                };
                if w == 0 {
                    return None;
                }
                h = ratio.height_for(w);
            } else {
                {
                    // Take a pixel off one side if that is enough (the
                    // larger valid area), else off both.
                    let one_side = [
                        (w.saturating_sub(1).max(1), h),
                        (w, h.saturating_sub(1).max(1)),
                    ]
                    .into_iter()
                    .filter(|&(cw, ch)| centred(cw, ch).is_some_and(|r| self.rect_is_valid(&r)))
                    .max_by_key(|&(cw, ch)| cw * ch);
                    (w, h) = one_side
                        .unwrap_or((w.saturating_sub(1).max(1), h.saturating_sub(1).max(1)));
                }
            }
        }
        None
    }

    /// `desired` brought back into the valid area, moving from `origin` (a
    /// valid rectangle of the same size) towards it: the whole move when it
    /// is valid, else the furthest whole-pixel position along the way that
    /// still is, else `origin`.
    pub(crate) fn clamp_move(&self, origin: Rect, desired: Rect) -> Rect {
        if self.rect_is_valid(&desired) {
            return desired;
        }
        let mut best = origin;
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        for _ in 0..40 {
            let t = f64::midpoint(lo, hi);
            let candidate = Rect {
                x: lerp_edge(origin.x, desired.x, t),
                y: lerp_edge(origin.y, desired.y, t),
                ..origin
            };
            if self.rect_is_valid(&candidate) {
                best = candidate;
                lo = t;
            } else {
                hi = t;
            }
        }
        best
    }

    /// `desired` (a resized rectangle) brought back into the valid area by
    /// moving its four edges back towards `origin`'s along the way. With an
    /// aspect `ratio` the edges cannot be interpolated without breaking it,
    /// so a rectangle that does not fit leaves `origin` as it was.
    pub(crate) fn clamp_resize(&self, origin: Rect, desired: Rect, ratio: Option<Ratio>) -> Rect {
        if self.rect_is_valid(&desired) {
            return desired;
        }
        if ratio.is_some() {
            return origin;
        }
        let mut best = origin;
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        for _ in 0..40 {
            let t = f64::midpoint(lo, hi);
            let left = lerp_edge(origin.x, desired.x, t);
            let top = lerp_edge(origin.y, desired.y, t);
            let right = lerp_edge(origin.right(), desired.right(), t);
            let bottom = lerp_edge(origin.bottom(), desired.bottom(), t);
            let candidate = (right > left && bottom > top)
                .then(|| Rect {
                    x: left,
                    y: top,
                    width: u32::try_from(right - left).unwrap_or(0),
                    height: u32::try_from(bottom - top).unwrap_or(0),
                })
                .filter(|r| !r.is_empty());
            if let Some(candidate) = candidate
                && self.rect_is_valid(&candidate)
            {
                best = candidate;
                lo = t;
            } else {
                hi = t;
            }
        }
        best
    }

    /// The axis-aligned box of source pixels (half-open, whole numbers) that
    /// can influence `rect`'s output: its rotated corners' box grown by the
    /// bicubic reach (two pixels) plus one for the floor.
    pub(crate) fn source_box(&self, rect: &Rect) -> Option<Rect> {
        let corners = self.corners(rect);
        let mut lo = (f64::INFINITY, f64::INFINITY);
        let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (x, y) in corners {
            lo = (lo.0.min(x), lo.1.min(y));
            hi = (hi.0.max(x), hi.1.max(y));
        }
        if !(lo.0.is_finite() && lo.1.is_finite() && hi.0.is_finite() && hi.1.is_finite()) {
            return None;
        }
        // Finite and bounded by the document range.
        #[allow(clippy::cast_possible_truncation)]
        let (x0, y0, x1, y1) = (
            (lo.0 - 3.0).floor() as i64,
            (lo.1 - 3.0).floor() as i64,
            (hi.0 + 3.0).ceil() as i64,
            (hi.1 + 3.0).ceil() as i64,
        );
        Some(Rect {
            x: x0,
            y: y0,
            width: u32::try_from(x1 - x0).ok()?,
            height: u32::try_from(y1 - y0).ok()?,
        })
    }
}

/// `a + (b - a) * t`, rounded to a whole number.
fn lerp_edge(a: i64, b: i64, t: f64) -> i64 {
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let v = (a as f64 + (b - a) as f64 * t).round() as i64;
    v
}

/// The half extents `(a, b)` maximising `a * b` subject to
/// `c*a + s*b <= x` and `s*a + c*b <= y` (`c = |cos|`, `s = |sin|`, both
/// non-negative, `c >= s`): the best of the intersection of the two limits
/// and of the unconstrained optimum on each limit, whichever is feasible.
fn best_half_extents(c: f64, s: f64, x: f64, y: f64) -> (f64, f64) {
    let feasible = |a: f64, b: f64| {
        a >= 0.0 && b >= 0.0 && c * a + s * b <= x + 1.0e-9 && s * a + c * b <= y + 1.0e-9
    };
    let mut candidates: Vec<(f64, f64)> = Vec::with_capacity(4);
    if s < 1.0e-12 {
        candidates.push((x / c, y / c));
    } else {
        let det = c * c - s * s;
        if det > 1.0e-12 {
            candidates.push(((c * x - s * y) / det, (c * y - s * x) / det));
        }
        candidates.push((x / (2.0 * c), x / (2.0 * s)));
        candidates.push((y / (2.0 * s), y / (2.0 * c)));
        // At exactly 45 degrees the two limits coincide.
        candidates.push((x.min(y) / (2.0 * c), x.min(y) / (2.0 * c)));
    }
    candidates
        .into_iter()
        .filter(|&(a, b)| feasible(a, b))
        .max_by(|p, q| (p.0 * p.1).total_cmp(&(q.0 * q.1)))
        .unwrap_or((0.0, 0.0))
}

// -- resampling ----------------------------------------------------------

/// The Catmull-Rom weights of the four taps `i - 1 ..= i + 2` for the
/// fractional position `t` in `0..1` between taps `i` and `i + 1`. They sum
/// to one.
pub(crate) fn catmull_rom_weights(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ]
}

/// The Catmull-Rom value at source point `p` (continuous, pixel `i` covering
/// `i .. i + 1`, so its centre is `i + 0.5`) of a plane whose texel `(x, y)`
/// is `fetch(x, y)` (the caller returns zeros outside its data).
pub(crate) fn sample<const N: usize>(
    fetch: &impl Fn(i64, i64) -> [f32; N],
    p: (f64, f64),
) -> [f32; N] {
    let (sx, sy) = (p.0 - 0.5, p.1 - 0.5);
    let (fx, fy) = (sx.floor(), sy.floor());
    #[allow(clippy::cast_possible_truncation)]
    let (ix, iy) = (fx as i64, fy as i64);
    #[allow(clippy::cast_possible_truncation)]
    let (wx, wy) = (
        catmull_rom_weights((sx - fx) as f32),
        catmull_rom_weights((sy - fy) as f32),
    );
    let mut out = [0.0_f32; N];
    for (dy, wy) in wy.iter().enumerate() {
        let y = iy + i64::try_from(dy).unwrap_or(0) - 1;
        let mut row = [0.0_f32; N];
        for (dx, wx) in wx.iter().enumerate() {
            let x = ix + i64::try_from(dx).unwrap_or(0) - 1;
            let tap = fetch(x, y);
            for (acc, v) in row.iter_mut().zip(tap) {
                *acc += v * wx;
            }
        }
        for (acc, v) in out.iter_mut().zip(row) {
            *acc += v * wy;
        }
    }
    out
}

/// A premultiplied RGBA result as a valid stored texel: alpha clamped to
/// `0..=1`, each colour to `0..=alpha`, anything not finite zero.
pub(crate) fn valid_texel(v: [f32; 4]) -> [f16; 4] {
    let clean = |x: f32| if x.is_finite() { x } else { 0.0 };
    let alpha = clean(v[3]).clamp(0.0, 1.0) + 0.0;
    // `+ 0.0` turns a negative zero into a positive one.
    let colour = |x: f32| clean(x).clamp(0.0, alpha) + 0.0;
    [
        f16::from_f32(colour(v[0])),
        f16::from_f32(colour(v[1])),
        f16::from_f32(colour(v[2])),
        f16::from_f32(alpha),
    ]
}

/// A dense window of one surface's source pixels in document coordinates.
pub(crate) struct PixelWindow {
    pub(crate) rect: Rect,
    /// Premultiplied RGBA, row-major over `rect`.
    pub(crate) texels: Vec<f16>,
}

impl PixelWindow {
    /// The tap at document texel `(x, y)`: transparent outside the window;
    /// a sample that is not finite counts as zero.
    pub(crate) fn tap(&self, x: i64, y: i64) -> [f32; 4] {
        if x < self.rect.x || y < self.rect.y || x >= self.rect.right() || y >= self.rect.bottom() {
            return [0.0; 4];
        }
        let (Ok(col), Ok(row), Ok(width)) = (
            usize::try_from(x - self.rect.x),
            usize::try_from(y - self.rect.y),
            usize::try_from(self.rect.width),
        ) else {
            return [0.0; 4];
        };
        let at = (row * width + col) * CHANNELS;
        match self.texels.get(at..at + CHANNELS) {
            Some([r, g, b, a]) => [r, g, b, a].map(|s| {
                let v = s.to_f32();
                if v.is_finite() { v } else { 0.0 }
            }),
            _ => [0.0; 4],
        }
    }
}

/// A dense window of coverage (one value per texel, `0..=1`) in document
/// coordinates.
pub(crate) struct CoverageWindow {
    pub(crate) rect: Rect,
    pub(crate) values: Vec<f32>,
}

impl CoverageWindow {
    /// The tap at document texel `(x, y)`: zero outside the window.
    pub(crate) fn tap(&self, x: i64, y: i64) -> [f32; 1] {
        if x < self.rect.x || y < self.rect.y || x >= self.rect.right() || y >= self.rect.bottom() {
            return [0.0];
        }
        let (Ok(col), Ok(row), Ok(width)) = (
            usize::try_from(x - self.rect.x),
            usize::try_from(y - self.rect.y),
            usize::try_from(self.rect.width),
        ) else {
            return [0.0];
        };
        [self.values.get(row * width + col).copied().unwrap_or(0.0)]
    }
}

/// What the resampler produces for one output tile: its id and 256 x 256
/// RGBA `f16` texels, or `None` for a tile that is all default.
pub(crate) type OutTile = (TileId, Vec<f16>);

/// The output tile grid of a `width x height` canvas.
pub(crate) fn tile_grid(width: u32, height: u32) -> (u32, u32) {
    (width.div_ceil(TILE), height.div_ceil(TILE))
}

/// The texels of output tile `(tx, ty)` for `rect`'s straighten: for each
/// texel inside the output canvas the Catmull-Rom sample of `fetch` at the
/// texel's source point, written by `store_texel` (which turns a sample into
/// the four stored `f16`s). Returns `None` when every texel is `default` (the
/// tile can stay absent).
pub(crate) fn resample_tile<const N: usize>(
    frame: &Frame,
    rect: &Rect,
    (tx, ty): (u32, u32),
    fetch: &impl Fn(i64, i64) -> [f32; N],
    store_texel: &impl Fn([f32; N]) -> [f16; 4],
    default: [f16; 4],
) -> Option<Vec<f16>> {
    let side = TILE as usize;
    let mut texels = vec![f16::from_f32(0.0); side * side * CHANNELS];
    let mut any = false;
    let x0 = tx * TILE;
    let y0 = ty * TILE;
    let columns = rect.width.saturating_sub(x0).min(TILE);
    let rows = rect.height.saturating_sub(y0).min(TILE);
    for row in 0..rows {
        for col in 0..columns {
            let p = frame.output_to_source(rect, f64::from(x0 + col), f64::from(y0 + row));
            let value = store_texel(sample(fetch, p));
            if value != default {
                any = true;
            }
            let at = (row as usize * side + col as usize) * CHANNELS;
            if let Some(slot) = texels.get_mut(at..at + CHANNELS) {
                slot.copy_from_slice(&value);
            }
        }
    }
    any.then_some(texels)
}
