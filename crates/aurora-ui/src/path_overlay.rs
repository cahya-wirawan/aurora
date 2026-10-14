//! The Pen tool's overlay and hit testing (0.233.0): the active path's
//! curve, its anchors and the selected anchor's handles, as [`OutlineRun`]s
//! for the selection-outline widget, and which part of the path a pointer
//! is over.
//!
//! **Feeding the canvas.** The runs go to the same widget list as the Crop
//! handles ([`aurora_widgets::widgets::set_selection_outline`] or the
//! marching-ants sync, whichever `aurora-app` composes for the frame): the
//! Pen and the Crop are never active together, so the app composes the
//! frame's runs as "ants + (Crop handles | path overlay)". No token is
//! added: the curve is a 1-physical-pixel line in `selection.ants_light`
//! on a 1-pixel rim in `selection.ants_dark`; every anchor is a small
//! square (`handle_px`, the app passes `spacing.xs` physical) of the same
//! pair; the **selected** anchor is filled in `accent.primary`
//! ([`OutlineTone::Accent`], a third tone the widget learnt in 0.233.0).
//! Only the selected anchor shows its handles: a thin line to each
//! non-zero handle and a small square at its end.
//!
//! The runs never overlap each other between tones (the rim is the
//! dilation of the line minus the line, and the squares carve their box
//! out of both), so the order the tones are painted in does not matter.
//! Everything is clipped to the canvas area. At most
//! [`MAX_DRAWN_ANCHORS`] anchor squares are drawn (the curve always is) and
//! at most [`MAX_LINE_PIXELS`] curve pixels, so a pathological path cannot
//! make a frame unbounded.

use std::collections::BTreeSet;

use aurora_doc::{FLATTEN_TOLERANCE, Path};
use aurora_widgets::widgets::{OutlineRun, OutlineTone};

use crate::CanvasView;
use crate::selection_outline::to_pixel;

/// The most anchor squares drawn per frame.
pub const MAX_DRAWN_ANCHORS: usize = 4096;
/// The most curve and handle-line pixels drawn per frame.
pub const MAX_LINE_PIXELS: usize = 200_000;

/// An anchor of a path by position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnchorRef {
    pub subpath: usize,
    pub anchor: usize,
}

/// Which part of the path a point is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathPart {
    Anchor(AnchorRef),
    /// The selected anchor's incoming handle.
    HandleIn(AnchorRef),
    /// The selected anchor's outgoing handle.
    HandleOut(AnchorRef),
}

fn anchor_of(path: &Path, at: AnchorRef) -> Option<&aurora_doc::Anchor> {
    path.subpaths.get(at.subpath)?.anchors.get(at.anchor)
}

/// An anchor's position in the canvas area's logical screen space.
#[must_use]
pub fn path_anchor_screen(path: &Path, at: AnchorRef, view: &CanvasView) -> Option<(f32, f32)> {
    anchor_of(path, at).map(|a| view.to_screen(a.point))
}

/// What the logical canvas-area point `pos` is over: the selected anchor's
/// handles first (outgoing, then incoming), then the anchors, the last one
/// drawn winning; each within `radius` logical pixels (`spacing.sm`).
/// Handles of unselected anchors are not drawn and so not hit.
#[must_use]
pub fn path_hit(
    path: &Path,
    selected: Option<AnchorRef>,
    view: &CanvasView,
    pos: (f32, f32),
    radius: f32,
) -> Option<PathPart> {
    let usable = radius.is_finite() && radius >= 0.0 && pos.0.is_finite() && pos.1.is_finite();
    if !usable {
        return None;
    }
    let near = |p: (f32, f32)| {
        let (dx, dy) = (p.0 - pos.0, p.1 - pos.1);
        dx.hypot(dy) <= radius
    };
    if let Some(at) = selected
        && let Some(a) = anchor_of(path, at)
    {
        if a.handle_out != (0.0, 0.0) && near(view.to_screen(a.out_position())) {
            return Some(PathPart::HandleOut(at));
        }
        if a.handle_in != (0.0, 0.0) && near(view.to_screen(a.in_position())) {
            return Some(PathPart::HandleIn(at));
        }
    }
    for (s, sub) in path.subpaths.iter().enumerate().rev() {
        for (i, a) in sub.anchors.iter().enumerate().rev() {
            if near(view.to_screen(a.point)) {
                return Some(PathPart::Anchor(AnchorRef {
                    subpath: s,
                    anchor: i,
                }));
            }
        }
    }
    None
}

type Pixel = (i64, i64);

/// A physical-pixel point of a document-space one.
fn to_physical(view: &CanvasView, doc: (f32, f32), scale: f64) -> (f64, f64) {
    let (x, y) = view.to_screen(doc);
    (f64::from(x) * scale, f64::from(y) * scale)
}

/// `a`-`b` clipped to `[lo, hi]` on both axes (Liang-Barsky), or `None`.
fn clip_segment(
    a: (f64, f64),
    b: (f64, f64),
    lo: (f64, f64),
    hi: (f64, f64),
) -> Option<((f64, f64), (f64, f64))> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, a.0 - lo.0),
        (dx, hi.0 - a.0),
        (-dy, a.1 - lo.1),
        (dy, hi.1 - a.1),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                if r > t1 {
                    return None;
                }
                t0 = t0.max(r);
            } else {
                if r < t0 {
                    return None;
                }
                t1 = t1.min(r);
            }
        }
    }
    Some((
        (a.0 + t0 * dx, a.1 + t0 * dy),
        (a.0 + t1 * dx, a.1 + t1 * dy),
    ))
}

/// Adds the pixels of the segment `a`-`b` (Bresenham on the rounded,
/// clipped end points) to `line`.
fn rasterise(line: &mut BTreeSet<Pixel>, a: (f64, f64), b: (f64, f64), clip: (i64, i64)) {
    // Pad by two pixels so the rim of a line along the edge is complete.
    #[allow(clippy::cast_precision_loss)]
    let hi = (clip.0 as f64 + 2.0, clip.1 as f64 + 2.0);
    let Some((a, b)) = clip_segment(a, b, (-2.0, -2.0), hi) else {
        return;
    };
    let (mut x, mut y) = (to_pixel(a.0), to_pixel(a.1));
    let (x1, y1) = (to_pixel(b.0), to_pixel(b.1));
    let (dx, dy) = ((x1 - x).abs(), -(y1 - y).abs());
    let (sx, sy) = (if x < x1 { 1 } else { -1 }, if y < y1 { 1 } else { -1 });
    let mut err = dx + dy;
    loop {
        if line.len() >= MAX_LINE_PIXELS {
            return;
        }
        line.insert((y, x));
        if x == x1 && y == y1 {
            return;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Appends the merged horizontal runs of `pixels` (`(y, x)`, sorted) cut
/// to `0..clip`.
fn push_pixels(
    out: &mut Vec<OutlineRun>,
    pixels: &BTreeSet<Pixel>,
    clip: (i64, i64),
    tone: OutlineTone,
) {
    let mut current: Option<(i64, i64, i64)> = None; // (y, x0, x1 exclusive)
    let flush = |run: Option<(i64, i64, i64)>, out: &mut Vec<OutlineRun>| {
        if let Some((y, x0, x1)) = run {
            push_clipped(out, (x0, y, x1 - x0, 1), clip, tone);
        }
    };
    for &(y, x) in pixels {
        match current {
            Some((cy, x0, x1)) if cy == y && x == x1 => current = Some((cy, x0, x + 1)),
            other => {
                flush(other, out);
                current = Some((y, x, x + 1));
            }
        }
    }
    flush(current, out);
}

/// Appends `(x, y, width, height)` cut to `0..clip`, if anything is left.
fn push_clipped(
    runs: &mut Vec<OutlineRun>,
    (x, y, width, height): (i64, i64, i64, i64),
    clip: (i64, i64),
    tone: OutlineTone,
) {
    let left = x.max(0);
    let top = y.max(0);
    let right = (x + width).min(clip.0);
    let bottom = (y + height).min(clip.1);
    if right <= left || bottom <= top {
        return;
    }
    let (Ok(left32), Ok(top32), Ok(w), Ok(h)) = (
        i32::try_from(left),
        i32::try_from(top),
        u32::try_from(right - left),
        u32::try_from(bottom - top),
    ) else {
        return;
    };
    runs.push(OutlineRun {
        x: left32,
        y: top32,
        width: w,
        height: h,
        tone,
    });
}

/// A square marker: its centre in physical pixels and how it is filled.
#[derive(Clone, Copy)]
struct Marker {
    centre: Pixel,
    tone: OutlineTone,
}

/// The overlay of `path` for one frame, in physical pixels from the canvas
/// area's top-left corner (see the module docs). `selected` is the anchor
/// drawn filled with its handles shown; `canvas_size` is the canvas area's
/// logical size; `handle_px` the side of an anchor square in physical
/// pixels (`0` draws no squares). Empty for a path with no anchor or a
/// canvas area with no size.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn path_overlay_runs(
    path: &Path,
    selected: Option<AnchorRef>,
    view: &CanvasView,
    canvas_size: (f32, f32),
    scale_factor: f32,
    handle_px: u32,
) -> Vec<OutlineRun> {
    if path.is_empty() {
        return Vec::new();
    }
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        f64::from(scale_factor)
    } else {
        1.0
    };
    let clip = (
        to_pixel(f64::from(canvas_size.0.max(0.0)) * scale),
        to_pixel(f64::from(canvas_size.1.max(0.0)) * scale),
    );
    if clip.0 <= 0 || clip.1 <= 0 {
        return Vec::new();
    }
    // Flatten finely enough for the screen: a quarter of a physical pixel
    // at most, and never coarser than the model's own tolerance.
    let zoom = f64::from(view.zoom().max(1.0e-6));
    #[allow(clippy::cast_possible_truncation)]
    let tolerance = ((0.25 / (zoom * scale)) as f32).clamp(0.001, FLATTEN_TOLERANCE);

    let mut line: BTreeSet<Pixel> = BTreeSet::new();
    for (poly, sub) in path
        .subpaths
        .iter()
        .map(|s| (s.flatten(tolerance), s))
        .filter(|(p, _)| !p.is_empty())
    {
        let mut points = poly;
        if sub.closed && points.len() > 1 {
            // `flatten` leaves the closing curve's last point (the first
            // anchor) off; the line has to end on it.
            if let Some(&first) = points.first() {
                points.push(first);
            }
        }
        let mut previous: Option<(f64, f64)> = None;
        for p in points {
            let here = to_physical(view, p, scale);
            if let Some(prev) = previous {
                rasterise(&mut line, prev, here, clip);
            } else if points_is_single(&sub.anchors) {
                rasterise(&mut line, here, here, clip);
            }
            previous = Some(here);
        }
    }

    // The selected anchor's handle lines.
    let mut markers: Vec<Marker> = Vec::new();
    if let Some(at) = selected
        && let Some(a) = anchor_of(path, at)
    {
        let anchor_px = to_physical(view, a.point, scale);
        for (offset, end) in [
            (a.handle_in, a.in_position()),
            (a.handle_out, a.out_position()),
        ] {
            if offset != (0.0, 0.0) {
                let end_px = to_physical(view, end, scale);
                rasterise(&mut line, anchor_px, end_px, clip);
                markers.push(Marker {
                    centre: (to_pixel(end_px.0), to_pixel(end_px.1)),
                    tone: OutlineTone::Light,
                });
            }
        }
    }
    // Anchor squares (visible ones only, bounded).
    let mut drawn = 0_usize;
    if handle_px > 0 {
        'subpaths: for (s, sub) in path.subpaths.iter().enumerate() {
            for (i, a) in sub.anchors.iter().enumerate() {
                let p = to_physical(view, a.point, scale);
                let centre = (to_pixel(p.0), to_pixel(p.1));
                let margin = i64::from(handle_px) + 2;
                if centre.0 < -margin
                    || centre.1 < -margin
                    || centre.0 > clip.0 + margin
                    || centre.1 > clip.1 + margin
                {
                    continue;
                }
                if drawn >= MAX_DRAWN_ANCHORS {
                    break 'subpaths;
                }
                drawn += 1;
                let is_selected = selected
                    == Some(AnchorRef {
                        subpath: s,
                        anchor: i,
                    });
                markers.push(Marker {
                    centre,
                    tone: if is_selected {
                        OutlineTone::Accent
                    } else {
                        OutlineTone::Light
                    },
                });
            }
        }
    }
    if handle_px == 0 {
        markers.clear();
    }

    // Squares carve their box (the square plus its rim) out of the line.
    let side = i64::from(handle_px);
    let half = side / 2;
    let mut boxed: BTreeSet<Pixel> = BTreeSet::new();
    for m in &markers {
        for y in (m.centre.1 - half - 1)..=(m.centre.1 - half + side) {
            for x in (m.centre.0 - half - 1)..=(m.centre.0 - half + side) {
                boxed.insert((y, x));
            }
        }
    }
    let mut core: BTreeSet<Pixel> = line.difference(&boxed).copied().collect();
    let mut rim: BTreeSet<Pixel> = BTreeSet::new();
    for &(y, x) in &line {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let p = (y + dy, x + dx);
                if !line.contains(&p) && !boxed.contains(&p) {
                    rim.insert(p);
                }
            }
        }
    }
    core.retain(|p| !boxed.contains(p));

    let mut runs = Vec::new();
    push_pixels(&mut runs, &rim, clip, OutlineTone::Dark);
    push_pixels(&mut runs, &core, clip, OutlineTone::Light);
    for m in &markers {
        let inner = (m.centre.0 - half, m.centre.1 - half, side, side);
        let ring = [
            (inner.0 - 1, inner.1 - 1, side + 2, 1),
            (inner.0 - 1, inner.1 + side, side + 2, 1),
            (inner.0 - 1, inner.1, 1, side),
            (inner.0 + side, inner.1, 1, side),
        ];
        for r in ring {
            push_clipped(&mut runs, r, clip, OutlineTone::Dark);
        }
        push_clipped(&mut runs, inner, clip, m.tone);
    }
    runs
}

/// A subpath of one anchor has no segment; it still shows its point.
fn points_is_single(anchors: &[aurora_doc::Anchor]) -> bool {
    anchors.len() == 1
}

#[cfg(test)]
#[path = "path_overlay_tests.rs"]
mod tests;
