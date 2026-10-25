//! The Crop tool (0.226.0): Photoshop's Crop, non-destructive variant.
//!
//! **What a crop is here.** The document is the rectangle
//! `(0, 0)-(canvas_w, canvas_h)` and every layer, mask and selection is
//! stored in document coordinates, with tile contents local to their layer
//! or mask. A crop to the integer rectangle `(x, y, w, h)` therefore
//! (a) sets the canvas to `w x h` and (b) shifts every pixel layer's
//! `bounds`, every mask's `bounds` and the active and saved selections by
//! `(-x, -y)` ([`aurora_doc::History::translate_all`],
//! [`aurora_doc::SelectionSet::translate`]), as ONE undo step labelled
//! "Crop". No pixel is touched: what lay outside the new canvas stays in
//! its layer (Photoshop's "Delete Cropped Pixels" is off) and is simply not
//! shown, composited or exported, as all layer content outside the canvas
//! already is. Undo shifts everything back exactly.
//!
//! **Smart objects.** A smart-object layer's recorded placement
//! (`SmartObject::bounds`, and the `SoLd` blocks behind it) is in document
//! coordinates and is deliberately NOT shifted by a crop: the PSD writer
//! keeps a smart object only while the layer's bounds still equal the
//! recorded ones. So a crop at origin `(0, 0)` keeps every smart object,
//! while ANY crop with `x > 0` or `y > 0` turns each smart-object layer into
//! ordinary pixels at the next PSD save, named in the "Save With Changes?"
//! report ("they were moved"); undoing the crop makes them smart objects
//! again (pinned in `psd_save_tests::crop_and_smart_objects`).
//!
//! **Cost.** Shifting a selection ([`aurora_doc::SelectionMask::translated`])
//! deep-copies a mask's coverage bytes (up to 64 Mpx), on the UI thread, for
//! every crop and every undo/redo of one; the copy drops the cached boundary
//! trace and breaks `Arc` sharing.
//!
//! This module is the pure part: the on-canvas rectangle ([`CropState`]:
//! create, move, resize by eight handles, clamp to the canvas, double-click),
//! the validation ([`validate`]) and the document operation
//! ([`crop_document`]) as free functions over the pieces, because `App`
//! needs a window.

#![allow(clippy::many_single_char_names)]

use std::time::{Duration, Instant};

use aurora_core::Rect;
use aurora_widgets::shortcut::{Modifiers, NamedKey};

use crate::{Key, LockRefusal, UndoOrder};

/// The palette / menu id of "Crop to Selection".
pub(crate) const COMMAND_CROP_TO_SELECTION: &str = "select.crop_to_selection";

/// The palette / menu title of "Crop to Selection".
#[cfg(any(test, target_os = "macos"))]
pub(crate) const COMMAND_TITLE: &str = "Crop to Selection";

/// The History row of a crop.
pub(crate) const CROP_LABEL: &str = "Crop";

/// Two presses inside the crop rectangle within this long make a
/// double-click, which commits (an interaction tolerance, not a style
/// value).
pub(crate) const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// A double-click's two presses may be this many document pixels apart at
/// most.
const DOUBLE_CLICK_SLOP: f32 = 4.0;

/// One of the eight handles of the crop rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CropHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl CropHandle {
    /// Every handle, in the order the overlay draws them
    /// ([`aurora_ui::crop_handle_runs`]).
    pub(crate) const ALL: [Self; 8] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Right,
        Self::BottomRight,
        Self::Bottom,
        Self::BottomLeft,
        Self::Left,
    ];

    /// Whether dragging the handle moves the left, right, top, bottom edge.
    const fn edges(self) -> (bool, bool, bool, bool) {
        match self {
            Self::TopLeft => (true, false, true, false),
            Self::Top => (false, false, true, false),
            Self::TopRight => (false, true, true, false),
            Self::Right => (false, true, false, false),
            Self::BottomRight => (false, true, false, true),
            Self::Bottom => (false, false, false, true),
            Self::BottomLeft => (true, false, false, true),
            Self::Left => (true, false, false, false),
        }
    }
}

/// What a document point is, relative to a crop rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CropHit {
    Handle(CropHandle),
    Inside,
    Outside,
}

/// Why a crop was not done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CropRefusal {
    /// The rectangle is empty, off the canvas or not a rectangle.
    Degenerate,
    /// The rectangle is the whole canvas: nothing to crop.
    NoChange,
    /// Quick Mask is on.
    QuickMask,
    /// A shifted layer or mask origin would leave the document range.
    OutOfRange,
    /// "Crop to Selection" with no selection.
    NeedsSelection,
}

impl CropRefusal {
    /// The calm status-bar notice for this refusal.
    pub(crate) const fn notice(self) -> LockRefusal {
        match self {
            Self::Degenerate => LockRefusal::CropDegenerate,
            Self::NoChange => LockRefusal::CropNoChange,
            Self::QuickMask => LockRefusal::QuickMaskLeaveFirst,
            Self::OutOfRange => LockRefusal::CropOutOfRange,
            Self::NeedsSelection => LockRefusal::CropNeedsSelection,
        }
    }
}

/// The crop rectangle the status bar reports: only while the Crop tool
/// has one.
pub(crate) fn status_rect(tool: aurora_ui::Tool, state: &CropState) -> Option<Rect> {
    (tool == aurora_ui::Tool::Crop)
        .then(|| state.rect())
        .flatten()
}

/// The canvas as a rectangle at the origin.
pub(crate) fn canvas_rect(canvas: (u32, u32)) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: canvas.0,
        height: canvas.1,
    }
}

/// A document coordinate as a whole pixel; `None` for NaN or infinity.
fn snap(value: f32) -> Option<i64> {
    if !value.is_finite() {
        return None;
    }
    // Finite and clamped to a range an `i64` holds exactly.
    #[allow(clippy::cast_possible_truncation)]
    Some(f64::from(value).round().clamp(-1.0e9, 1.0e9) as i64)
}

/// A rectangle from edges `left <= right`, `top <= bottom` (already clamped);
/// `None` when it has no area.
fn from_edges(left: i64, top: i64, right: i64, bottom: i64) -> Option<Rect> {
    let width = u32::try_from(right.checked_sub(left)?).ok()?;
    let height = u32::try_from(bottom.checked_sub(top)?).ok()?;
    (width > 0 && height > 0).then_some(Rect {
        x: left,
        y: top,
        width,
        height,
    })
}

/// The rectangle between two document points, snapped to whole pixels and
/// clamped to the canvas; `None` when a point is not finite or the
/// rectangle has no area.
pub(crate) fn rect_from_points(a: (f32, f32), b: (f32, f32), canvas: (u32, u32)) -> Option<Rect> {
    let (w, h) = (i64::from(canvas.0), i64::from(canvas.1));
    let (ax, ay, bx, by) = (snap(a.0)?, snap(a.1)?, snap(b.0)?, snap(b.1)?);
    let (ax, bx) = (ax.clamp(0, w), bx.clamp(0, w));
    let (ay, by) = (ay.clamp(0, h), by.clamp(0, h));
    from_edges(ax.min(bx), ay.min(by), ax.max(bx), ay.max(by))
}

/// `origin` moved by `(dx, dy)` document pixels (rounded), kept whole inside
/// the canvas; a rectangle larger than the canvas stays at the canvas.
pub(crate) fn moved(origin: Rect, dx: f32, dy: f32, canvas: (u32, u32)) -> Rect {
    let (dx, dy) = (snap(dx).unwrap_or(0), snap(dy).unwrap_or(0));
    let room_x = (i64::from(canvas.0) - i64::from(origin.width)).max(0);
    let room_y = (i64::from(canvas.1) - i64::from(origin.height)).max(0);
    Rect {
        x: origin.x.saturating_add(dx).clamp(0, room_x),
        y: origin.y.saturating_add(dy).clamp(0, room_y),
        ..origin
    }
}

/// `origin` with `handle` dragged to document point `to`: the edges the
/// handle owns follow the point (snapped, clamped to the canvas); crossing
/// the opposite edge flips the rectangle, and it never gets thinner than one
/// pixel. A non-finite point leaves it unchanged.
pub(crate) fn resized(
    origin: Rect,
    handle: CropHandle,
    to: (f32, f32),
    canvas: (u32, u32),
) -> Rect {
    let (Some(px), Some(py)) = (snap(to.0), snap(to.1)) else {
        return origin;
    };
    let (w, h) = (i64::from(canvas.0), i64::from(canvas.1));
    let (px, py) = (px.clamp(0, w), py.clamp(0, h));
    let (mut l, mut t, mut r, mut b) = (origin.x, origin.y, origin.right(), origin.bottom());
    let (el, er, et, eb) = handle.edges();
    if el {
        l = px;
    }
    if er {
        r = px;
    }
    if et {
        t = py;
    }
    if eb {
        b = py;
    }
    let (mut l, mut r) = (l.min(r), l.max(r));
    let (mut t, mut b) = (t.min(b), t.max(b));
    if r == l {
        if r < w {
            r += 1;
        } else {
            l -= 1;
        }
    }
    if b == t {
        if b < h {
            b += 1;
        } else {
            t -= 1;
        }
    }
    from_edges(l, t, r, b).unwrap_or(origin)
}

/// What the document point `p` is relative to `rect`; a handle wins within
/// `radius` document pixels (corners before edge midpoints, the nearest
/// first), then the inside, else outside.
pub(crate) fn hit_test(rect: &Rect, p: (f32, f32), radius: f32) -> CropHit {
    if !p.0.is_finite() || !p.1.is_finite() {
        return CropHit::Outside;
    }
    let radius = if radius.is_finite() && radius > 0.0 {
        radius
    } else {
        0.0
    };
    #[allow(clippy::cast_precision_loss)]
    let (l, t, r, b) = (
        rect.x as f32,
        rect.y as f32,
        rect.right() as f32,
        rect.bottom() as f32,
    );
    let (cx, cy) = (f32::midpoint(l, r), f32::midpoint(t, b));
    let mut best: Option<(f32, CropHandle)> = None;
    for handle in CropHandle::ALL {
        let (hx, hy) = match handle {
            CropHandle::TopLeft => (l, t),
            CropHandle::Top => (cx, t),
            CropHandle::TopRight => (r, t),
            CropHandle::Right => (r, cy),
            CropHandle::BottomRight => (r, b),
            CropHandle::Bottom => (cx, b),
            CropHandle::BottomLeft => (l, b),
            CropHandle::Left => (l, cy),
        };
        let distance = (p.0 - hx).abs().max((p.1 - hy).abs());
        // Strictly nearer wins, so the corners (listed first) win ties.
        if distance <= radius && best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, handle));
        }
    }
    if let Some((_, handle)) = best {
        return CropHit::Handle(handle);
    }
    if p.0 >= l && p.0 < r && p.1 >= t && p.1 < b {
        CropHit::Inside
    } else {
        CropHit::Outside
    }
}

/// The pointer cursor the Crop tool wants over the canvas (0.231.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum CropCursor {
    /// The window's ordinary pointer: another tool, off the canvas, or a
    /// modal in the way.
    #[default]
    Default,
    /// No rectangle yet, or the pointer is outside it: a drag makes one.
    Crosshair,
    /// Inside the rectangle: a drag moves it.
    Move,
    /// Over the top or bottom handle.
    ResizeNs,
    /// Over the left or right handle.
    ResizeEw,
    /// Over the top-right or bottom-left handle.
    ResizeNeSw,
    /// Over the top-left or bottom-right handle.
    ResizeNwSe,
}

impl CropCursor {
    /// The winit icon for the kind: the one place the kinds meet winit.
    pub(crate) const fn icon(self) -> winit::window::CursorIcon {
        use winit::window::CursorIcon;
        match self {
            Self::Default => CursorIcon::Default,
            Self::Crosshair => CursorIcon::Crosshair,
            Self::Move => CursorIcon::Move,
            Self::ResizeNs => CursorIcon::NsResize,
            Self::ResizeEw => CursorIcon::EwResize,
            Self::ResizeNeSw => CursorIcon::NeswResize,
            Self::ResizeNwSe => CursorIcon::NwseResize,
        }
    }
}

/// The cursor for `tool` with the pointer at document point `pointer`
/// (`None` when it is not over the canvas, or something modal is open):
/// the ordinary one unless the Crop tool is active, then a crosshair with
/// no rectangle or outside it, a move cursor inside it and the matching
/// resize cursor over a handle (the same [`hit_test`] a press uses, with
/// the same `radius` in document pixels).
pub(crate) fn cursor_for(
    tool: aurora_ui::Tool,
    rect: Option<&Rect>,
    pointer: Option<(f32, f32)>,
    radius: f32,
) -> CropCursor {
    let (aurora_ui::Tool::Crop, Some(pointer)) = (tool, pointer) else {
        return CropCursor::Default;
    };
    let Some(rect) = rect else {
        return CropCursor::Crosshair;
    };
    match hit_test(rect, pointer, radius) {
        CropHit::Outside => CropCursor::Crosshair,
        CropHit::Inside => CropCursor::Move,
        CropHit::Handle(CropHandle::Top | CropHandle::Bottom) => CropCursor::ResizeNs,
        CropHit::Handle(CropHandle::Left | CropHandle::Right) => CropCursor::ResizeEw,
        CropHit::Handle(CropHandle::TopRight | CropHandle::BottomLeft) => CropCursor::ResizeNeSw,
        CropHit::Handle(CropHandle::TopLeft | CropHandle::BottomRight) => CropCursor::ResizeNwSe,
    }
}

/// An arrow key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arrow {
    Left,
    Right,
    Up,
    Down,
}

/// How far a plain arrow key nudges, in document pixels.
pub(crate) const NUDGE_STEP: u32 = 1;

/// How far `Shift` + an arrow key nudges.
pub(crate) const NUDGE_STEP_LARGE: u32 = 10;

/// One keyboard nudge of the crop rectangle (0.231.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Nudge {
    pub(crate) arrow: Arrow,
    /// Document pixels (1 or 10).
    pub(crate) step: u32,
    /// `Alt` held: resize the bottom-right corner instead of moving.
    pub(crate) resize: bool,
}

/// The nudge an arrow key with `modifiers` means: arrow = 1 px move,
/// `Shift` = 10, `Alt` = resize the bottom-right edges instead. Any key
/// that is not an arrow, and any arrow with `Control` or `Meta` held (those
/// belong to the shortcuts), is `None`.
pub(crate) fn nudge_for_key(key: Key, modifiers: Modifiers) -> Option<Nudge> {
    if modifiers.control || modifiers.meta {
        return None;
    }
    let arrow = match key {
        Key::Named(NamedKey::ArrowLeft) => Arrow::Left,
        Key::Named(NamedKey::ArrowRight) => Arrow::Right,
        Key::Named(NamedKey::ArrowUp) => Arrow::Up,
        Key::Named(NamedKey::ArrowDown) => Arrow::Down,
        _ => return None,
    };
    Some(Nudge {
        arrow,
        step: if modifiers.shift {
            NUDGE_STEP_LARGE
        } else {
            NUDGE_STEP
        },
        resize: modifiers.alt,
    })
}

/// `rect` after `nudge`, inside the canvas. A move keeps the size and
/// stops at the canvas edges; a resize moves the right edge (Left/Right)
/// or the bottom edge (Up/Down) by the step, never below one pixel and
/// never past the canvas, leaving the origin where it is.
pub(crate) fn nudged(rect: Rect, nudge: Nudge, canvas: (u32, u32)) -> Rect {
    let step = i64::from(nudge.step);
    let (dx, dy) = match nudge.arrow {
        Arrow::Left => (-step, 0),
        Arrow::Right => (step, 0),
        Arrow::Up => (0, -step),
        Arrow::Down => (0, step),
    };
    if nudge.resize {
        let room_w = (i64::from(canvas.0) - rect.x).max(1);
        let room_h = (i64::from(canvas.1) - rect.y).max(1);
        let width = (i64::from(rect.width) + dx).clamp(1, room_w);
        let height = (i64::from(rect.height) + dy).clamp(1, room_h);
        Rect {
            width: u32::try_from(width).unwrap_or(rect.width),
            height: u32::try_from(height).unwrap_or(rect.height),
            ..rect
        }
    } else {
        let room_x = (i64::from(canvas.0) - i64::from(rect.width)).max(0);
        let room_y = (i64::from(canvas.1) - i64::from(rect.height)).max(0);
        Rect {
            x: rect.x.saturating_add(dx).clamp(0, room_x),
            y: rect.y.saturating_add(dy).clamp(0, room_y),
            ..rect
        }
    }
}

// -- Aspect-ratio presets (0.236.0) -----------------------------------------
//
// With a ratio the crop rectangle keeps `width : height == ratio` as a pair
// of WHOLE pixels. The model, for every gesture:
//
// * **Size.** The pointer fixes one axis (the *dominant* one: the axis whose
//   pointer distance is larger in ratio units, so the rectangle always
//   covers the box between the fixed point and the pointer) and the other is
//   derived by rounding to the nearest pixel (halves up, never below 1).
//   Deriving `h` from `w` leaves `|w * rh - h * rw| <= rw / 2`, deriving `w`
//   from `h` leaves it `<= rh / 2`, so the relative ratio error
//   `|(w / h) / (rw / rh) - 1|` is at most `1 / min(w, h)` whenever neither
//   side was floored at 1 px or cut by the canvas (a bound the tests sweep).
//   An exact ratio is hit exactly whenever the dominant size is a multiple
//   of the ratio's width (or height) in lowest terms.
// * **Clamping.** A rectangle that would leave the canvas is SHRUNK to the
//   largest size of the same ratio that fits, never distorted; the fixed
//   corner (or edge) stays where it is.
// * **Floors.** At least 1 x 1 px; a canvas too small for the ratio gives
//   the nearest fit (down to 1 px), as best effort.

/// An aspect ratio `w : h` in lowest terms, both sides above zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ratio {
    pub(crate) w: u32,
    pub(crate) h: u32,
}

const fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Ratio {
    /// `w : h` in lowest terms; `None` when a side is zero.
    pub(crate) const fn new(w: u32, h: u32) -> Option<Self> {
        if w == 0 || h == 0 {
            return None;
        }
        let g = gcd(w, h);
        Some(Self { w: w / g, h: h / g })
    }

    /// The portrait of a landscape ratio and the other way round.
    pub(crate) const fn swapped(self) -> Self {
        Self {
            w: self.h,
            h: self.w,
        }
    }

    /// The height that goes with `width`: nearest whole pixel, halves up,
    /// at least 1.
    pub(crate) fn height_for(self, width: u64) -> u64 {
        let (w, h) = (u128::from(self.w), u128::from(self.h));
        let rounded = (2 * u128::from(width) * h + w) / (2 * w);
        u64::try_from(rounded).unwrap_or(u64::MAX).max(1)
    }

    /// The width that goes with `height` (see [`Self::height_for`]).
    pub(crate) fn width_for(self, height: u64) -> u64 {
        self.swapped().height_for(height)
    }
}

/// The ratio the options bar's choice means for a canvas: `None` for Free
/// (and for an Original canvas with no area); Portrait swaps the sides
/// (1:1 is its own swap).
pub(crate) fn ratio_for(
    choice: aurora_ui::CropRatio,
    portrait: bool,
    canvas: (u32, u32),
) -> Option<Ratio> {
    let landscape = match choice {
        aurora_ui::CropRatio::Free => return None,
        aurora_ui::CropRatio::Original => Ratio::new(canvas.0, canvas.1)?,
        other => {
            let (w, h) = other.fixed()?;
            Ratio::new(w, h)?
        }
    };
    Some(if portrait {
        landscape.swapped()
    } else {
        landscape
    })
}

/// The palette id of `Crop Ratio: <choice>`.
pub(crate) const fn ratio_command_id(choice: aurora_ui::CropRatio) -> &'static str {
    match choice {
        aurora_ui::CropRatio::Free => "crop.ratio.free",
        aurora_ui::CropRatio::Original => "crop.ratio.original",
        aurora_ui::CropRatio::Square => "crop.ratio.1_1",
        aurora_ui::CropRatio::ThreeTwo => "crop.ratio.3_2",
        aurora_ui::CropRatio::FourThree => "crop.ratio.4_3",
        aurora_ui::CropRatio::FiveFour => "crop.ratio.5_4",
        aurora_ui::CropRatio::SixteenNine => "crop.ratio.16_9",
        aurora_ui::CropRatio::SixteenTen => "crop.ratio.16_10",
    }
}

/// The palette title of `Crop Ratio: <choice>`.
#[cfg(test)]
pub(crate) fn ratio_command_title(choice: aurora_ui::CropRatio) -> String {
    format!("Crop Ratio: {}", choice.label())
}

/// The palette id and title of "Crop: Swap Orientation".
pub(crate) const COMMAND_SWAP_ORIENTATION: &str = "crop.swap_orientation";
#[cfg(test)]
pub(crate) const SWAP_ORIENTATION_TITLE: &str = "Crop: Swap Orientation";

/// Which side the pointer fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

/// The whole-pixel size of ratio `ratio` that has `desired` (at least 1)
/// along `axis` and fits `max` (both above zero); when it does not fit it
/// is shrunk, keeping the ratio, to the largest that does (down to the
/// 1 px floor).
fn fit_size(ratio: Ratio, axis: Axis, desired: u64, max: (u64, u64)) -> (u64, u64) {
    let desired = desired.max(1);
    let (mut w, mut h) = match axis {
        Axis::X => (desired, ratio.height_for(desired)),
        Axis::Y => (ratio.width_for(desired), desired),
    };
    if w > max.0 || h > max.1 {
        // The widest width whose height still fits: floor(max_h * rw / rh).
        let by_height =
            u64::try_from(u128::from(max.1) * u128::from(ratio.w) / u128::from(ratio.h))
                .unwrap_or(u64::MAX);
        w = w.min(max.0).min(by_height).max(1);
        h = ratio.height_for(w).min(max.1.max(1));
    }
    (w, h)
}

/// The rectangle whose corner `anchor` is fixed, grown towards `pointer`
/// (both whole pixels, the anchor inside the canvas) with `ratio`. The
/// side of the anchor it grows to follows the pointer (`default_sign` at an
/// exact tie); it covers the box between the two, shrunk to fit the canvas.
/// `None` when the pointer is on the anchor (`min_one` false), or the
/// canvas has no room on the chosen side.
fn grow_from_corner(
    anchor: (i64, i64),
    pointer: (i64, i64),
    default_sign: (i64, i64),
    ratio: Ratio,
    canvas: (u32, u32),
    min_one: bool,
) -> Option<Rect> {
    let (cw, ch) = (i64::from(canvas.0), i64::from(canvas.1));
    let (dx, dy) = (pointer.0 - anchor.0, pointer.1 - anchor.1);
    let sx = match dx {
        d if d < 0 => -1,
        d if d > 0 => 1,
        _ => default_sign.0,
    };
    let sy = match dy {
        d if d < 0 => -1,
        d if d > 0 => 1,
        _ => default_sign.1,
    };
    let room_x = if sx > 0 { cw - anchor.0 } else { anchor.0 };
    let room_y = if sy > 0 { ch - anchor.1 } else { anchor.1 };
    if room_x <= 0 || room_y <= 0 {
        return None;
    }
    let (mut adx, ady) = (dx.unsigned_abs(), dy.unsigned_abs());
    if adx == 0 && ady == 0 {
        if !min_one {
            return None;
        }
        adx = 1;
    }
    // Width-dominant when the pointer is further out in ratio units.
    let width_first =
        u128::from(adx) * u128::from(ratio.h) >= u128::from(ady) * u128::from(ratio.w);
    let (axis, desired) = if width_first {
        (Axis::X, adx)
    } else {
        (Axis::Y, ady)
    };
    let max = (room_x.unsigned_abs(), room_y.unsigned_abs());
    let (w, h) = fit_size(ratio, axis, desired, max);
    let (w, h) = (i64::try_from(w).ok()?, i64::try_from(h).ok()?);
    let x = if sx > 0 { anchor.0 } else { anchor.0 - w };
    let y = if sy > 0 { anchor.1 } else { anchor.1 - h };
    from_edges(x, y, x + w, y + h)
}

/// [`rect_from_points`] with an aspect ratio: the press point `a` stays a
/// corner, the rectangle grows towards `b` ([`grow_from_corner`]).
pub(crate) fn rect_from_points_ratio(
    a: (f32, f32),
    b: (f32, f32),
    canvas: (u32, u32),
    ratio: Ratio,
) -> Option<Rect> {
    let (w, h) = (i64::from(canvas.0), i64::from(canvas.1));
    let anchor = (snap(a.0)?.clamp(0, w), snap(a.1)?.clamp(0, h));
    let pointer = (snap(b.0)?, snap(b.1)?);
    grow_from_corner(anchor, pointer, (1, 1), ratio, canvas, false)
}

/// The rectangle of an edge handle on the left/right side: the opposite
/// edge stays, the dragged edge follows `px` (crossing flips), the height is
/// derived about the rectangle's centre line and slid back inside the
/// canvas if it would stick out.
fn edge_resize_horizontal(
    origin: Rect,
    right: bool,
    px: i64,
    canvas: (u32, u32),
    ratio: Ratio,
) -> Option<Rect> {
    let (cw, ch) = (i64::from(canvas.0), i64::from(canvas.1));
    let fixed = if right { origin.x } else { origin.right() };
    let dx = px - fixed;
    let sx = match dx {
        d if d < 0 => -1,
        d if d > 0 => 1,
        _ if right => 1,
        _ => -1,
    };
    let room = if sx > 0 { cw - fixed } else { fixed };
    if room <= 0 || ch <= 0 {
        return None;
    }
    let (w, h) = fit_size(
        ratio,
        Axis::X,
        dx.unsigned_abs(),
        (room.unsigned_abs(), ch.unsigned_abs()),
    );
    let (w, h) = (i64::try_from(w).ok()?, i64::try_from(h).ok()?);
    // Twice the centre line, so an odd difference rounds down.
    let centre2 = origin.y + origin.bottom();
    let y = (centre2 - h).div_euclid(2).clamp(0, (ch - h).max(0));
    let x = if sx > 0 { fixed } else { fixed - w };
    from_edges(x, y, x + w, y + h)
}

const fn transposed(rect: Rect) -> Rect {
    Rect {
        x: rect.y,
        y: rect.x,
        width: rect.height,
        height: rect.width,
    }
}

/// [`resized`] with an aspect ratio. A corner handle is the opposite
/// corner fixed and the rectangle grown towards the pointer
/// ([`grow_from_corner`]); an edge handle moves its edge and derives the
/// other size about the centre line. Where the canvas leaves no room (the
/// pointer across a fixed edge that lies on the canvas edge) or the point
/// is not finite, the rectangle is left as it was.
pub(crate) fn resized_ratio(
    origin: Rect,
    handle: CropHandle,
    to: (f32, f32),
    canvas: (u32, u32),
    ratio: Ratio,
) -> Rect {
    let (Some(px), Some(py)) = (snap(to.0), snap(to.1)) else {
        return origin;
    };
    let (l, t, r, b) = (origin.x, origin.y, origin.right(), origin.bottom());
    let corner = |fixed: (i64, i64), sign: (i64, i64)| {
        grow_from_corner(fixed, (px, py), sign, ratio, canvas, true)
    };
    let resized = match handle {
        CropHandle::TopLeft => corner((r, b), (-1, -1)),
        CropHandle::TopRight => corner((l, b), (1, -1)),
        CropHandle::BottomRight => corner((l, t), (1, 1)),
        CropHandle::BottomLeft => corner((r, t), (-1, 1)),
        CropHandle::Left => edge_resize_horizontal(origin, false, px, canvas, ratio),
        CropHandle::Right => edge_resize_horizontal(origin, true, px, canvas, ratio),
        CropHandle::Top | CropHandle::Bottom => edge_resize_horizontal(
            transposed(origin),
            handle == CropHandle::Bottom,
            py,
            (canvas.1, canvas.0),
            ratio.swapped(),
        )
        .map(transposed),
    };
    resized.unwrap_or(origin)
}

/// `rect` re-fitted to a new `ratio`: the same centre (slid back inside the
/// canvas if needed) and the area as close as whole pixels allow, shrunk to
/// the canvas if it does not fit.
pub(crate) fn refit(rect: Rect, ratio: Ratio, canvas: (u32, u32)) -> Rect {
    let area = f64::from(rect.width) * f64::from(rect.height);
    let width = (area * f64::from(ratio.w) / f64::from(ratio.h)).sqrt();
    // A positive finite number below 2^53: the cast cannot truncate.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let desired = width.round().max(1.0) as u64;
    let (cw, ch) = (i64::from(canvas.0), i64::from(canvas.1));
    if cw <= 0 || ch <= 0 {
        return rect;
    }
    let (w, h) = fit_size(
        ratio,
        Axis::X,
        desired,
        (cw.unsigned_abs(), ch.unsigned_abs()),
    );
    let (Ok(w), Ok(h)) = (i64::try_from(w), i64::try_from(h)) else {
        return rect;
    };
    let x = (rect.x * 2 + i64::from(rect.width) - w)
        .div_euclid(2)
        .clamp(0, (cw - w).max(0));
    let y = (rect.y * 2 + i64::from(rect.height) - h)
        .div_euclid(2)
        .clamp(0, (ch - h).max(0));
    from_edges(x, y, x + w, y + h).unwrap_or(rect)
}

/// [`nudged`] with an aspect ratio: a move is unchanged, a resize (`Alt` +
/// arrow) changes the width (Left/Right) or the height (Up/Down) by the
/// step, keeps the top-left corner and derives the other side by
/// rounding to the nearest pixel (halves up), shrinking if the canvas
/// stops it.
pub(crate) fn nudged_ratio(
    rect: Rect,
    nudge: Nudge,
    canvas: (u32, u32),
    ratio: Option<Ratio>,
) -> Rect {
    let Some(ratio) = ratio.filter(|_| nudge.resize) else {
        return nudged(rect, nudge, canvas);
    };
    let step = i64::from(nudge.step);
    let room_w = (i64::from(canvas.0) - rect.x).max(1);
    let room_h = (i64::from(canvas.1) - rect.y).max(1);
    let max = (room_w.unsigned_abs(), room_h.unsigned_abs());
    let (w, h) = match nudge.arrow {
        Arrow::Left | Arrow::Right => {
            let dx = if nudge.arrow == Arrow::Left {
                -step
            } else {
                step
            };
            let width = (i64::from(rect.width) + dx).clamp(1, room_w);
            fit_size(ratio, Axis::X, width.unsigned_abs(), max)
        }
        Arrow::Up | Arrow::Down => {
            let dy = if nudge.arrow == Arrow::Up {
                -step
            } else {
                step
            };
            let height = (i64::from(rect.height) + dy).clamp(1, room_h);
            fit_size(ratio, Axis::Y, height.unsigned_abs(), max)
        }
    };
    Rect {
        width: u32::try_from(w).unwrap_or(rect.width),
        height: u32::try_from(h).unwrap_or(rect.height),
        ..rect
    }
}

/// The gesture a press began.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Gesture {
    /// Dragging out a new rectangle from `anchor`.
    Create { anchor: (f32, f32) },
    /// Moving the rectangle: the press point and the rectangle then.
    Move { grab: (f32, f32), origin: Rect },
    /// Resizing by `handle` from `origin`.
    Resize { handle: CropHandle, origin: Rect },
}

/// What a press did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Press {
    /// A drag began.
    Started,
    /// A double-click inside the rectangle: commit it.
    Commit(Rect),
}

/// The Crop tool's on-canvas state: the rectangle (if any), the live drag
/// and the last press (for double-click).
#[derive(Debug, Clone, Default)]
pub(crate) struct CropState {
    rect: Option<Rect>,
    gesture: Option<Gesture>,
    last_press: Option<(Instant, (f32, f32))>,
    /// The document and canvas the rectangle belongs to.
    owner: Option<(crate::document_session::DocumentId, (u32, u32))>,
    /// The aspect ratio the rectangle is held to (0.236.0); `None` is free.
    /// Survives [`Self::cancel`]: it is a setting, not a gesture.
    ratio: Option<Ratio>,
}

impl CropState {
    /// The current rectangle.
    pub(crate) fn rect(&self) -> Option<Rect> {
        self.rect
    }

    /// Whether a drag is in progress.
    pub(crate) fn dragging(&self) -> bool {
        self.gesture.is_some()
    }

    /// Forgets the rectangle and any drag (Escape, a tool or document
    /// switch): nothing is committed.
    pub(crate) fn cancel(&mut self) {
        *self = Self {
            ratio: self.ratio,
            ..Self::default()
        };
    }

    /// The aspect ratio in force.
    #[cfg(test)]
    pub(crate) fn ratio(&self) -> Option<Ratio> {
        self.ratio
    }

    /// Holds the rectangle to `ratio` from now on (`None` frees it). A
    /// change re-fits an existing rectangle ([`refit`]; Free leaves it as
    /// it is) and ends a live drag, whose origin belonged to the old
    /// ratio. Returns whether anything changed.
    pub(crate) fn set_ratio(&mut self, ratio: Option<Ratio>, canvas: (u32, u32)) -> bool {
        if self.ratio == ratio {
            return false;
        }
        self.ratio = ratio;
        self.gesture = None;
        if let (Some(ratio), Some(rect)) = (ratio, self.rect) {
            self.rect = Some(refit(rect, ratio, canvas));
        }
        true
    }

    /// Whether the state belongs to another document or canvas size than
    /// `(id, canvas)` (then it must be cancelled).
    pub(crate) fn is_stale(
        &self,
        id: crate::document_session::DocumentId,
        canvas: (u32, u32),
    ) -> bool {
        self.rect.is_some() && self.owner != Some((id, canvas))
    }

    /// A primary press at document point `p`. Over a handle it starts a
    /// resize, inside the rectangle a move (a second press within
    /// [`DOUBLE_CLICK`] commits), elsewhere a new rectangle (the old one is
    /// dropped; a click without a drag leaves none).
    pub(crate) fn press(
        &mut self,
        p: (f32, f32),
        radius: f32,
        now: Instant,
        id: crate::document_session::DocumentId,
        canvas: (u32, u32),
    ) -> Press {
        let double = self.last_press.is_some_and(|(then, at)| {
            now.saturating_duration_since(then) <= DOUBLE_CLICK
                && (p.0 - at.0).abs().max((p.1 - at.1).abs()) <= DOUBLE_CLICK_SLOP
        });
        self.last_press = Some((now, p));
        self.owner = Some((id, canvas));
        let hit = self
            .rect
            .map_or(CropHit::Outside, |rect| hit_test(&rect, p, radius));
        match (self.rect, hit) {
            (Some(rect), CropHit::Handle(handle)) => {
                self.gesture = Some(Gesture::Resize {
                    handle,
                    origin: rect,
                });
            }
            (Some(rect), CropHit::Inside) => {
                if double {
                    self.gesture = None;
                    self.last_press = None;
                    return Press::Commit(rect);
                }
                self.gesture = Some(Gesture::Move {
                    grab: p,
                    origin: rect,
                });
            }
            _ => {
                self.rect = None;
                self.gesture = Some(Gesture::Create { anchor: p });
            }
        }
        Press::Started
    }

    /// The pointer moved to `p` during a drag.
    pub(crate) fn drag(&mut self, p: (f32, f32), canvas: (u32, u32)) {
        match self.gesture {
            Some(Gesture::Create { anchor }) => {
                self.rect = match self.ratio {
                    Some(ratio) => rect_from_points_ratio(anchor, p, canvas, ratio),
                    None => rect_from_points(anchor, p, canvas),
                };
            }
            Some(Gesture::Move { grab, origin }) => {
                self.rect = Some(moved(origin, p.0 - grab.0, p.1 - grab.1, canvas));
            }
            Some(Gesture::Resize { handle, origin }) => {
                self.rect = Some(match self.ratio {
                    Some(ratio) => resized_ratio(origin, handle, p, canvas, ratio),
                    None => resized(origin, handle, p, canvas),
                });
            }
            None => {}
        }
    }

    /// Applies a keyboard [`Nudge`] to the rectangle; returns whether the
    /// state could take it (there is a rectangle and no drag is live).
    pub(crate) fn nudge(&mut self, nudge: Nudge, canvas: (u32, u32)) -> bool {
        match (self.rect, self.gesture) {
            (Some(rect), None) => {
                self.rect = Some(nudged_ratio(rect, nudge, canvas, self.ratio));
                true
            }
            _ => false,
        }
    }

    /// The pointer was released: the drag ends, the rectangle stays.
    pub(crate) fn release(&mut self) {
        self.gesture = None;
    }
}

/// A crop rectangle checked against the canvas: whole pixels inside it, at
/// least 1 x 1, and not the whole canvas.
pub(crate) fn validate(rect: Rect, canvas: (u32, u32)) -> Result<Rect, CropRefusal> {
    let canvas = canvas_rect(canvas);
    if rect.is_empty() || canvas.is_empty() {
        return Err(CropRefusal::Degenerate);
    }
    let left = rect.x.max(0);
    let top = rect.y.max(0);
    let right = rect.right().min(canvas.right());
    let bottom = rect.bottom().min(canvas.bottom());
    let clipped = from_edges(left, top, right, bottom).ok_or(CropRefusal::Degenerate)?;
    if clipped == canvas {
        return Err(CropRefusal::NoChange);
    }
    Ok(clipped)
}

/// The bounds of the active selection, clipped to the canvas, as the crop
/// rectangle of "Crop to Selection". Pixels outside a non-rectangular
/// selection are NOT deleted: only the bounds are used.
pub(crate) fn selection_crop_rect(
    selection: Option<&aurora_doc::Selection>,
    canvas: (u32, u32),
) -> Result<Rect, CropRefusal> {
    let selection = selection.ok_or(CropRefusal::NeedsSelection)?;
    // An inverted selection has no finite box that is "the selected area".
    if selection.inverted {
        return Err(CropRefusal::NeedsSelection);
    }
    validate(selection.bounds, canvas)
}

/// One crop as the unified undo order keeps it (the shift and both canvas
/// sizes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CropStep {
    pub(crate) dx: i64,
    pub(crate) dy: i64,
    pub(crate) before: (u32, u32),
    pub(crate) after: (u32, u32),
}

/// What an undo or redo of a crop leaves for the owner of the live document
/// to apply ([`apply_pending_crop`]): the total shift to apply to the
/// selections and the canvas size now in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingCrop {
    pub(crate) dx: i64,
    pub(crate) dy: i64,
    pub(crate) canvas: (u32, u32),
}

/// Crops the document to `rect` as one "Crop" undo step. All or nothing:
/// on an error nothing has changed and nothing is recorded.
///
/// # Errors
///
/// A [`CropRefusal`]: Quick Mask on, a degenerate or whole-canvas
/// rectangle, or a shifted origin out of the document range.
#[allow(clippy::too_many_arguments)]
pub(crate) fn crop_document(
    layers: &mut aurora_doc::LayerTree,
    history: &mut aurora_doc::History,
    pixel_history: &mut aurora_brush::PixelHistory,
    undo_order: &mut UndoOrder,
    selection: &mut aurora_doc::SelectionSet,
    canvas_size: &mut (u32, u32),
    quick_mask_on: bool,
    rect: Rect,
) -> Result<(), CropRefusal> {
    if quick_mask_on {
        return Err(CropRefusal::QuickMask);
    }
    let rect = validate(rect, *canvas_size)?;
    let (dx, dy) = (-rect.x, -rect.y);
    selection
        .translate(dx, dy)
        .map_err(|_| CropRefusal::OutOfRange)?;
    if history.translate_all(layers, dx, dy).is_err() {
        // The forward shift succeeded, so its inverse cannot overflow.
        let _ = selection.translate(-dx, -dy);
        return Err(CropRefusal::OutOfRange);
    }
    let step = CropStep {
        dx,
        dy,
        before: *canvas_size,
        after: (rect.width, rect.height),
    };
    *canvas_size = step.after;
    undo_order.record_crop(step, history, pixel_history);
    Ok(())
}

/// Applies the canvas size and selection shift an undo, redo or History jump
/// of a crop left pending ([`UndoOrder::take_pending_crop`]); returns
/// whether there was one. Run it BEFORE
/// [`crate::selection_view::apply_pending_selection`]: a selection step
/// undone in the same jump already holds coordinates of its own era.
pub(crate) fn apply_pending_crop(
    undo_order: &mut UndoOrder,
    canvas_size: &mut (u32, u32),
    selection: &mut aurora_doc::SelectionSet,
) -> bool {
    let Some(pending) = undo_order.take_pending_crop() else {
        return false;
    };
    if selection.translate(pending.dx, pending.dy).is_err() {
        // `SelectionSet::translate` is all-or-nothing, so the set is
        // unchanged here and every selection in it (the active one AND the
        // saved ones) would sit at the wrong place after the canvas change.
        // Only a shift that overflows `i64` gets here, which no shift that
        // was applied once and is now undone, and no tool-made selection,
        // can produce; the consistent answer is to drop them all rather
        // than keep some at stale coordinates.
        *selection = aurora_doc::SelectionSet::new();
    }
    *canvas_size = pending.canvas;
    true
}
