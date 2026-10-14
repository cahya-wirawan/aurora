//! Pen editing helpers (0.234.0): pure functions on [`Subpath`]s that the
//! Pen tool uses to find a segment under the pointer, split a Bezier
//! segment exactly (de Casteljau) and toggle an anchor between corner and
//! smooth.

use crate::path::{Anchor, Subpath};

type Pt = (f32, f32);

fn lerp(a: Pt, b: Pt, t: f32) -> Pt {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

fn dist(a: Pt, b: Pt) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// A point on a segment of a subpath: the segment runs from anchor
/// `segment` to anchor `segment + 1` (the last anchor of a closed subpath
/// to the first), `t` is the curve parameter and `distance` how far the
/// query point is from the curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentHit {
    pub subpath: usize,
    pub segment: usize,
    pub t: f32,
    pub distance: f32,
}

/// Splits the cubic from `a` to `b` at `t` (de Casteljau), returning `a`
/// with its outgoing handle shortened, the new anchor, and `b` with its
/// incoming handle shortened. The two halves trace exactly the original
/// curve. A straight segment (no handle at either end) splits into a
/// straight segment: the new anchor is a corner.
#[must_use]
pub fn split_segment(a: &Anchor, b: &Anchor, t: f32) -> (Anchor, Anchor, Anchor) {
    let t = t.clamp(0.0, 1.0);
    if a.handle_out == (0.0, 0.0) && b.handle_in == (0.0, 0.0) {
        let p = lerp(a.point, b.point, t);
        return (*a, Anchor::corner(p.0, p.1), *b);
    }
    let (p0, p1, p2, p3) = (a.point, a.out_position(), b.in_position(), b.point);
    let (q0, q1, q2) = (lerp(p0, p1, t), lerp(p1, p2, t), lerp(p2, p3, t));
    let (r0, r1) = (lerp(q0, q1, t), lerp(q1, q2, t));
    let s = lerp(r0, r1, t);
    let mut first = *a;
    first.handle_out = (q0.0 - p0.0, q0.1 - p0.1);
    let middle = Anchor {
        point: s,
        handle_in: (r0.0 - s.0, r0.1 - s.1),
        handle_out: (r1.0 - s.0, r1.1 - s.1),
    };
    let mut last = *b;
    last.handle_in = (q2.0 - p3.0, q2.1 - p3.1);
    (first, middle, last)
}

fn curve_point(a: &Anchor, b: &Anchor, t: f32) -> Pt {
    if a.handle_out == (0.0, 0.0) && b.handle_in == (0.0, 0.0) {
        return lerp(a.point, b.point, t);
    }
    let (p0, p1, p2, p3) = (a.point, a.out_position(), b.in_position(), b.point);
    let (q0, q1, q2) = (lerp(p0, p1, t), lerp(p1, p2, t), lerp(p2, p3, t));
    lerp(lerp(q0, q1, t), lerp(q1, q2, t), t)
}

/// The closest approach of `point` to the chord `p -> q`: the parameter
/// along it in `0..=1` and the distance.
fn nearest_on_chord(p: Pt, q: Pt, point: Pt) -> (f32, f32) {
    let (dx, dy) = (q.0 - p.0, q.1 - p.1);
    let len2 = dx * dx + dy * dy;
    let u = if len2 <= 0.0 {
        0.0
    } else {
        (((point.0 - p.0) * dx + (point.1 - p.1) * dy) / len2).clamp(0.0, 1.0)
    };
    (u, dist(lerp(p, q, u), point))
}

/// Samples per segment when looking for the nearest point of a curve.
const HIT_SAMPLES: u32 = 48;

/// The segment of `subpaths` closest to `point` within `radius`, or `None`.
/// An anchor itself is the caller's business: this only measures curves.
#[must_use]
pub fn nearest_segment(subpaths: &[Subpath], point: Pt, radius: f32) -> Option<SegmentHit> {
    let mut best: Option<SegmentHit> = None;
    for (si, sub) in subpaths.iter().enumerate() {
        let count_all = sub.anchors.len();
        if count_all < 2 {
            continue;
        }
        let count = if sub.closed { count_all } else { count_all - 1 };
        for seg in 0..count {
            let (Some(from), Some(to)) =
                (sub.anchors.get(seg), sub.anchors.get((seg + 1) % count_all))
            else {
                continue;
            };
            let mut prev = from.point;
            for step in 1..=HIT_SAMPLES {
                #[allow(clippy::cast_precision_loss)]
                let t1 = step as f32 / HIT_SAMPLES as f32;
                let next = if step == HIT_SAMPLES {
                    to.point
                } else {
                    curve_point(from, to, t1)
                };
                let (u, d) = nearest_on_chord(prev, next, point);
                #[allow(clippy::cast_precision_loss)]
                let t0 = (step - 1) as f32 / HIT_SAMPLES as f32;
                let t = t0 + (t1 - t0) * u;
                if d <= radius && best.is_none_or(|h| d < h.distance) {
                    best = Some(SegmentHit {
                        subpath: si,
                        segment: seg,
                        t,
                        distance: d,
                    });
                }
                prev = next;
            }
        }
    }
    best
}

/// Inserts an anchor at `hit` (splitting the segment exactly). Returns the
/// new anchor's index, or `None` when `hit` does not name a segment or the
/// subpath already holds [`crate::MAX_ANCHORS_PER_SUBPATH`] anchors.
pub fn insert_anchor(subpaths: &mut [Subpath], hit: &SegmentHit) -> Option<usize> {
    let sub = subpaths.get_mut(hit.subpath)?;
    let n = sub.anchors.len();
    if !(2..crate::MAX_ANCHORS_PER_SUBPATH).contains(&n)
        || hit.segment >= n
        || (!sub.closed && hit.segment + 1 >= n)
    {
        return None;
    }
    let next = (hit.segment + 1) % n;
    let (first, middle, last) =
        split_segment(sub.anchors.get(hit.segment)?, sub.anchors.get(next)?, hit.t);
    *sub.anchors.get_mut(hit.segment)? = first;
    *sub.anchors.get_mut(next)? = last;
    // After the first anchor of the segment; for a closing segment that is
    // the end of the list, which is the same closed shape.
    let at = hit.segment + 1;
    sub.anchors.insert(at, middle);
    Some(at)
}

/// Toggles anchor `index` of `sub` between corner and smooth. A corner
/// becomes smooth with handles along the direction from its previous to
/// its next neighbour, each a third of the distance to that neighbour; a
/// smooth (or half-handled) anchor becomes a corner. Returns whether the
/// anchor changed (a lone anchor has no direction and stays).
pub fn toggle_corner_smooth(sub: &mut Subpath, index: usize) -> bool {
    let n = sub.anchors.len();
    let Some(anchor) = sub.anchors.get(index).copied() else {
        return false;
    };
    if !anchor.is_corner() {
        if let Some(a) = sub.anchors.get_mut(index) {
            a.handle_in = (0.0, 0.0);
            a.handle_out = (0.0, 0.0);
        }
        return true;
    }
    let neighbour = |forward: bool| -> Option<Pt> {
        let j = if forward {
            if index + 1 < n {
                index + 1
            } else if sub.closed {
                0
            } else {
                return None;
            }
        } else if index > 0 {
            index - 1
        } else if sub.closed {
            n - 1
        } else {
            return None;
        };
        if j == index {
            return None;
        }
        sub.anchors.get(j).map(|a| a.point)
    };
    let (prev, next) = (neighbour(false), neighbour(true));
    if prev.is_none() && next.is_none() {
        return false;
    }
    let from = prev.unwrap_or(anchor.point);
    let to = next.unwrap_or(anchor.point);
    let tangent = (to.0 - from.0, to.1 - from.1);
    let len = tangent.0.hypot(tangent.1);
    if len <= 0.0 || !len.is_finite() {
        return false;
    }
    let unit = (tangent.0 / len, tangent.1 / len);
    let out_len = next.map_or(0.0, |p| dist(p, anchor.point) / 3.0);
    let in_len = prev.map_or(0.0, |p| dist(p, anchor.point) / 3.0);
    if let Some(a) = sub.anchors.get_mut(index) {
        a.handle_out = (unit.0 * out_len, unit.1 * out_len);
        a.handle_in = (-unit.0 * in_len, -unit.1 * in_len);
    }
    true
}

/// The subpath traversed backwards (anchors reversed, handles swapped).
#[must_use]
pub fn reversed(sub: &Subpath) -> Subpath {
    Subpath {
        anchors: sub
            .anchors
            .iter()
            .rev()
            .map(|a| Anchor {
                point: a.point,
                handle_in: a.handle_out,
                handle_out: a.handle_in,
            })
            .collect(),
        closed: sub.closed,
    }
}

#[cfg(test)]
#[path = "path_edit_tests.rs"]
mod tests;
