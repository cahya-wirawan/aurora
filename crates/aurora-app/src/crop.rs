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

use crate::{LockRefusal, UndoOrder};

/// The palette / menu id of "Crop to Selection".
pub(crate) const COMMAND_CROP_TO_SELECTION: &str = "select.crop_to_selection";

/// The palette / menu title of "Crop to Selection".
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
        *self = Self::default();
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
            Some(Gesture::Create { anchor }) => self.rect = rect_from_points(anchor, p, canvas),
            Some(Gesture::Move { grab, origin }) => {
                self.rect = Some(moved(origin, p.0 - grab.0, p.1 - grab.1, canvas));
            }
            Some(Gesture::Resize { handle, origin }) => {
                self.rect = Some(resized(origin, handle, p, canvas));
            }
            None => {}
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
