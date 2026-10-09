//! PSD vector masks (0.150.0): the `vmsk`/`vsms` tagged block parsed and
//! rasterised to anti-aliased coverage, so the mask can be applied as an
//! ordinary Aurora pixel mask.
//!
//! Aurora has no vector-mask model, so the path is *converted*: what
//! opens is coverage at the document's own resolution, and the path
//! itself is not kept (the import report says so).
//!
//! # Semantics followed (psd-tools 1.17.4, `composite/vector.py`)
//!
//! - **Records.** The block is a version (`3`), a flags word (bit 0
//!   invert, bit 1 not linked, bit 2 disabled) and then 26-byte path
//!   records, each a `u16` selector and 24 bytes: `0`/`3` start a closed
//!   or open subpath and give its knot count and its shape operation;
//!   `1`/`2`/`4`/`5` are knots (linked or unlinked — the difference is
//!   editing behaviour only); `6` is the path fill rule record, `7` the
//!   clipboard record (both carry nothing the raster needs); `8` is the
//!   initial fill record.
//! - **Knots** hold three points — the control point *preceding* the
//!   anchor, the anchor, the control point *leaving* it — each as two
//!   signed 8.24 fixed-point numbers, **vertical first**, normalised to
//!   the document's height and width. Between knots `a` and `b` the edge
//!   is the cubic Bézier `a.anchor, a.leaving, b.preceding, b.anchor`; a
//!   closed subpath also joins its last knot to its first, an open one is
//!   filled as if closed by a straight line (as a scanline fill does).
//!   A subpath with fewer than two knots draws nothing.
//! - **Fill rule.** Each subpath is filled on its own with the non-zero
//!   winding rule (AGG's default, which psd-tools' `aggdraw` uses), and
//!   the subpaths of one *group* are composited with "over"
//!   (`m + (1 - m) c`), as successive brush fills on one canvas are.
//! - **Groups and operations.** A subpath whose operation is `-1` joins
//!   the group before it; any other starts a group with that operation.
//!   Groups combine in order: `0` exclude (`m + p - 2mp`), `1` combine
//!   (`m + p - mp`), `2` subtract (`max(0, m - p)`), `3` intersect
//!   (`m p`); for subtract and intersect, the *first* group first inverts
//!   the starting value. Any other operation is skipped. A `-1` on the
//!   very first subpath (psd-tools raises `IndexError` there) is read as
//!   combine.
//! - **Initial fill.** The starting value is `1` only when the initial
//!   fill record is non-zero *and* there are no subpaths at all —
//!   psd-tools' own rule — else `0`. The result is clamped to `[0, 1]`.
//! - **Invert** (flags bit 0) gives `1 - m`. psd-tools does **not** apply
//!   it (its `_draw_path` never reads the flag); Aurora does, as
//!   Photoshop's own mask setting says. No corpus file sets it.
//!
//! # Bounded work
//!
//! A path is untrusted input: at most [`MAX_VECTOR_RECORDS`] records are
//! read; each cubic is flattened into at most [`MAX_CURVE_STEPS`] line
//! segments by Wang's formula at [`FLATTEN_TOLERANCE`] px, and at most
//! [`MAX_FLATTENED_SEGMENTS`] segments in all — counted before
//! flattening (`segment_total`) and charged, with one unit per parsed
//! record, to the file's work budget, so once that is spent nothing more
//! is parsed or flattened. Before any raster buffer
//! exists, the raster rectangle (the paths' bounding box, clipped to the
//! canvas) is charged [`RASTER_PIXEL_CHARGE`] budget pixels per pixel
//! against the file's share of [`super::PIXEL_BUDGET`], and the scanline
//! work `work_for` counts — every edge crossing times `1 + log2(edges)`
//! for the per-sub-scanline sort, a step per sub-scanline and two passes
//! over each subpath's box, three whole-raster passes per group that
//! draws anything, two more for the result — is capped at
//! [`MAX_RASTER_WORK`] per mask and [`MAX_FILE_VECTOR_WORK`] per file
//! ([`Budget`]). Groups that draw nothing are folded into a constant and
//! cost no per-pixel pass. Exceeding any bound is
//! [`VectorFailure::TooLarge`], never a refused file.
//!
//! The buffers sized by the input — the subpath and knot lists, each
//! flattened polygon and edge list, the raster planes and the result —
//! are reserved fallibly (`try_reserve`). The per-group index lists and
//! each fill's active-edge and crossing lists grow infallibly; they are
//! bounded by the record cap and by one polygon's edge count.
//!
//! Measured worst case (release, this repository's RTX 3090 machine —
//! the work is CPU only): see `PLAN.md`, addendum 0.150.0, "Review
//! revision".

// Every `f64 -> usize` cast below is of a value already clamped to a
// non-negative, in-range span.
#![allow(clippy::cast_sign_loss)]
// Bézier and scanline arithmetic reads best in its textbook letters.
#![allow(clippy::many_single_char_names)]

use aurora_core::Rect;
use half::f16;

/// The most 26-byte path records one vector mask may hold. Photoshop
/// paths are a few dozen; 65,536 is far past any hand-drawn shape.
pub(super) const MAX_VECTOR_RECORDS: usize = 1 << 16;

/// The most line segments one cubic is flattened into.
pub(super) const MAX_CURVE_STEPS: u32 = 256;

/// The most line segments one whole vector mask may flatten into.
pub(super) const MAX_FLATTENED_SEGMENTS: usize = 1 << 20;

/// The largest distance, in document pixels, a flattened segment may
/// stray from the true curve (Wang's bound). A twentieth of a pixel is
/// below what one anti-aliasing level can show.
pub(super) const FLATTEN_TOLERANCE: f64 = 0.05;

/// Sub-scanlines per pixel row. Horizontal coverage is exact (each span
/// adds its true fractional width); vertical coverage is sampled at this
/// many evenly spaced heights, so an edge pixel's coverage is within
/// `1 / (2 × 16)` of its exact area.
pub(super) const SUBSAMPLES: u32 = 16;

/// The most scanline work one vector mask may cost, as `work_for`
/// counts it: every edge crossing times `1 + log2(edges)` (the
/// per-sub-scanline sort), a step per sub-scanline and two passes over
/// each subpath's box, three whole-raster passes per group that draws,
/// and three more (two planes and the result).
pub(super) const MAX_RASTER_WORK: u64 = 1 << 28;

const RECORD_LEN: usize = 26;

/// Why a vector mask is not applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VectorFailure {
    /// The block does not parse (wrong version, unknown record, a
    /// subpath longer than the records left, too many records).
    Unreadable,
    /// It parses, but converting it would exceed a work or memory bound.
    TooLarge,
}

/// A point, normalised: `x` is a fraction of the document's width, `y`
/// of its height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Knot {
    pub preceding: Point,
    pub anchor: Point,
    pub leaving: Point,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Subpath {
    pub closed: bool,
    pub operation: i16,
    pub knots: Vec<Knot>,
}

/// A parsed `vmsk`/`vsms` block.
#[derive(Clone, Debug, Default, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // independent flags
pub(super) struct VectorPath {
    pub invert: bool,
    pub not_link: bool,
    pub disable: bool,
    /// The initial fill record's value was non-zero.
    pub initial_fill: bool,
    pub subpaths: Vec<Subpath>,
}

fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let pair = bytes.get(at..at + 2)?;
    Some(u16::from_be_bytes([*pair.first()?, *pair.get(1)?]))
}

fn be_i32(bytes: &[u8], at: usize) -> Option<i32> {
    let quad: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(i32::from_be_bytes(quad))
}

/// One 8.24 fixed-point pair, vertical first.
fn fixed_point(bytes: &[u8], at: usize) -> Option<Point> {
    let scale = f64::from(1_u32 << 24);
    let y = f64::from(be_i32(bytes, at)?) / scale;
    let x = f64::from(be_i32(bytes, at + 4)?) / scale;
    Some(Point { x, y })
}

/// Parses a `vmsk`/`vsms` block. Trailing bytes too few for a whole
/// record are ignored (padding); everything else that does not parse is
/// [`VectorFailure::Unreadable`].
pub(super) fn parse(data: &[u8]) -> Result<VectorPath, VectorFailure> {
    let version = data
        .get(0..4)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_be_bytes)
        .ok_or(VectorFailure::Unreadable)?;
    let flags = data
        .get(4..8)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_be_bytes)
        .ok_or(VectorFailure::Unreadable)?;
    if version != 3 {
        return Err(VectorFailure::Unreadable);
    }
    let body = data.get(8..).unwrap_or(&[]);
    let count = body.len() / RECORD_LEN;
    if count > MAX_VECTOR_RECORDS {
        return Err(VectorFailure::Unreadable);
    }
    let mut path = VectorPath {
        invert: flags & 1 != 0,
        not_link: flags & 2 != 0,
        disable: flags & 4 != 0,
        initial_fill: false,
        subpaths: Vec::new(),
    };
    // Knots still owed to the last subpath.
    let mut pending: usize = 0;
    for record in body.chunks_exact(RECORD_LEN) {
        let selector = be_u16(record, 0).ok_or(VectorFailure::Unreadable)?;
        match selector {
            0 | 3 => {
                if pending != 0 {
                    // A new subpath before the last one's knots: the
                    // count lied.
                    return Err(VectorFailure::Unreadable);
                }
                let knots = usize::from(be_u16(record, 2).ok_or(VectorFailure::Unreadable)?);
                let operation = be_u16(record, 4).ok_or(VectorFailure::Unreadable)?;
                let mut list = Vec::new();
                list.try_reserve_exact(knots.min(count))
                    .map_err(|_| VectorFailure::TooLarge)?;
                path.subpaths
                    .try_reserve(1)
                    .map_err(|_| VectorFailure::TooLarge)?;
                path.subpaths.push(Subpath {
                    closed: selector == 0,
                    operation: i16::from_be_bytes(operation.to_be_bytes()),
                    knots: list,
                });
                pending = knots;
            }
            1 | 2 | 4 | 5 => {
                let knot = Knot {
                    preceding: fixed_point(record, 2).ok_or(VectorFailure::Unreadable)?,
                    anchor: fixed_point(record, 10).ok_or(VectorFailure::Unreadable)?,
                    leaving: fixed_point(record, 18).ok_or(VectorFailure::Unreadable)?,
                };
                if pending > 0
                    && let Some(subpath) = path.subpaths.last_mut()
                {
                    subpath
                        .knots
                        .try_reserve(1)
                        .map_err(|_| VectorFailure::TooLarge)?;
                    subpath.knots.push(knot);
                    pending -= 1;
                }
                // A knot outside any subpath is ignored, as psd-tools'
                // `VectorMask.paths` ignores it.
            }
            6 | 7 => {
                // Inside a subpath's count psd-tools stores the record as
                // one of its items; it carries no geometry either way.
                pending = pending.saturating_sub(1);
            }
            8 => {
                // Inside a subpath's count psd-tools stores it as one of
                // the subpath's items, where `VectorMask` never reads it.
                if pending == 0 {
                    path.initial_fill = be_u16(record, 2).ok_or(VectorFailure::Unreadable)? != 0;
                } else {
                    pending -= 1;
                }
            }
            _ => return Err(VectorFailure::Unreadable),
        }
    }
    if pending != 0 {
        return Err(VectorFailure::Unreadable);
    }
    Ok(path)
}

/// A rasterised vector mask over a document-space rectangle.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Raster {
    /// Where [`Self::coverage`] lies, in document coordinates — the
    /// paths' bounding box (grown by a pixel) clipped to the canvas.
    /// Empty when nothing is drawn.
    pub bounds: Rect,
    /// Row-major over `bounds`, `0.0..=1.0`, invert applied.
    pub coverage: Vec<f16>,
    /// The (constant) coverage everywhere outside `bounds`: `true` for
    /// shown (`1`), `false` for hidden (`0`).
    pub outside: bool,
    /// The budget pixels this raster was charged (refunded by the caller
    /// when it ends up not using it).
    pub charged: u64,
}

/// One edge of a flattened polygon, `y0 < y1`, in raster-local pixels.
#[derive(Clone, Copy, Debug)]
struct Edge {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    winding: i32,
}

fn to_pixels(p: Point, width: f64, height: f64) -> (f64, f64) {
    (p.x * width, p.y * height)
}

/// Wang's bound: the uniform step count that keeps a cubic within
/// [`FLATTEN_TOLERANCE`] of its chords, capped at [`MAX_CURVE_STEPS`].
fn curve_steps(p: [(f64, f64); 4]) -> u32 {
    let [p0, p1, p2, p3] = p;
    // Handles on their own anchors (a corner-to-corner edge): the curve
    // *is* the chord, traversed at a varying speed — one segment, exactly.
    if p1 == p0 && p2 == p3 {
        return 1;
    }
    let second = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| {
        let dx = a.0 - 2.0 * b.0 + c.0;
        let dy = a.1 - 2.0 * b.1 + c.1;
        dx.hypot(dy)
    };
    let m = second(p0, p1, p2).max(second(p1, p2, p3));
    let n = (0.75 * m / FLATTEN_TOLERANCE).sqrt().ceil();
    if n.is_finite() && n >= 1.0 {
        n.min(f64::from(MAX_CURVE_STEPS)) as u32
    } else {
        1
    }
}

fn cubic(p: [(f64, f64); 4], t: f64) -> (f64, f64) {
    let [p0, p1, p2, p3] = p;
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (
        a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
        a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
    )
}

/// One subpath as a closed polygon in document pixels, or `None` when it
/// has fewer than two knots. Its segment count is `segment_total`'s,
/// checked against [`MAX_FLATTENED_SEGMENTS`] before any subpath is
/// flattened.
fn flatten(
    subpath: &Subpath,
    width: f64,
    height: f64,
) -> Result<Option<Vec<(f64, f64)>>, VectorFailure> {
    let knots = &subpath.knots;
    if knots.len() < 2 {
        return Ok(None);
    }
    let pairs = knots.len() - usize::from(!subpath.closed);
    let mut steps = Vec::new();
    steps
        .try_reserve_exact(pairs)
        .map_err(|_| VectorFailure::TooLarge)?;
    let mut total: usize = 1;
    for i in 0..pairs {
        let (Some(a), Some(b)) = (knots.get(i), knots.get((i + 1) % knots.len())) else {
            return Err(VectorFailure::Unreadable);
        };
        let p = [
            to_pixels(a.anchor, width, height),
            to_pixels(a.leaving, width, height),
            to_pixels(b.preceding, width, height),
            to_pixels(b.anchor, width, height),
        ];
        let n = curve_steps(p);
        total = total.saturating_add(n as usize);
        steps.push((p, n));
    }
    let mut points = Vec::new();
    points
        .try_reserve_exact(total)
        .map_err(|_| VectorFailure::TooLarge)?;
    if let Some(first) = knots.first() {
        points.push(to_pixels(first.anchor, width, height));
    }
    for (p, n) in steps {
        for i in 1..=n {
            points.push(cubic(p, f64::from(i) / f64::from(n)));
        }
    }
    Ok(Some(points))
}

/// The value the combination rules give where every group's plane is
/// `0` — everywhere outside the paths — and, applied per pixel, inside.
fn combine(mut m: f32, p: f32, operation: i16, first: bool) -> f32 {
    match operation {
        0 => m + p - 2.0 * m * p,
        1 => m + p - m * p,
        2 => {
            if first {
                m = 1.0 - m;
            }
            (m - p).max(0.0)
        }
        3 => {
            if first {
                m = 1.0 - m;
            }
            m * p
        }
        _ => m,
    }
}

/// The line segments `path` flattens into — exactly what `flatten`
/// counts, without allocating — saturating.
fn segment_total(path: &VectorPath, width: f64, height: f64) -> u64 {
    let mut total: u64 = 0;
    for subpath in &path.subpaths {
        let knots = &subpath.knots;
        if knots.len() < 2 {
            continue;
        }
        let pairs = knots.len() - usize::from(!subpath.closed);
        total = total.saturating_add(1);
        for i in 0..pairs {
            let (Some(a), Some(b)) = (knots.get(i), knots.get((i + 1) % knots.len())) else {
                continue;
            };
            let p = [
                to_pixels(a.anchor, width, height),
                to_pixels(a.leaving, width, height),
                to_pixels(b.preceding, width, height),
                to_pixels(b.anchor, width, height),
            ];
            total = total.saturating_add(u64::from(curve_steps(p)));
        }
    }
    total
}

/// Subpaths grouped by operation: `(operation, member indices)`.
fn groups(path: &VectorPath) -> Vec<(i16, Vec<usize>)> {
    let mut out: Vec<(i16, Vec<usize>)> = Vec::new();
    for (i, subpath) in path.subpaths.iter().enumerate() {
        match (subpath.operation, out.last_mut()) {
            (-1, Some((_, members))) => members.push(i),
            (-1, None) => out.push((1, vec![i])),
            (op, _) => out.push((op, vec![i])),
        }
    }
    out
}

/// What vector-mask conversion may still spend, shared by every vector
/// mask of one file ([`super::decode`] threads one through the whole
/// layer tree): pixels of [`super::PIXEL_BUDGET`] — a raster pixel is
/// charged [`RASTER_PIXEL_CHARGE`] of them — and scanline work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Budget {
    pub pixels: u64,
    pub work: u64,
}

/// Budget pixels charged per raster pixel. One budget pixel is 8 bytes
/// (`f16` RGBA); a raster pixel peaks at 10 (two `f32` working planes
/// and the `f16` result), so it is charged two (16 bytes).
pub(super) const RASTER_PIXEL_CHARGE: u64 = 2;

/// Work units charged per flattened segment. Measured (release): one
/// segment — two Wang bounds, a cubic evaluation, the push, the box —
/// costs ~8 ns, about eight times a scanline work unit, so a file whose
/// whole budget goes on flattening ends near the same ~2 s as one whose
/// budget goes on scanline fills.
pub(super) const SEGMENT_CHARGE: u64 = 8;

/// The most scanline work all of one file's vector masks may cost
/// together — four masks at [`MAX_RASTER_WORK`].
pub(super) const MAX_FILE_VECTOR_WORK: u64 = 1 << 30;

/// The work [`rasterize`] will do for `polygons` over a `rw × rh`
/// raster, in "operations": per subpath, every edge crossing (one per
/// edge per sub-scanline it spans) times `1 + log2(edges)` for the
/// per-sub-scanline sort, plus a sub-scanline step per row of its box and
/// two passes over its box (accumulate, reset); per non-empty group,
/// three passes over the raster (materialise, clear, combine); and three
/// more (the two planes' allocation, the result). Box sides are rounded
/// up by a pixel or two, so a box is charged within its own area plus
/// its perimeter, never less. A group with no drawable subpath costs
/// nothing per pixel: it is folded into a constant (see [`rasterize`]).
fn work_for(edges: &[Option<Vec<Edge>>], nonempty_groups: u64, rw: usize, rh: usize) -> u64 {
    let len = (rw as u64).saturating_mul(rh as u64);
    let scale = f64::from(SUBSAMPLES);
    let mut work = len.saturating_mul(3);
    work = work.saturating_add(nonempty_groups.saturating_mul(len).saturating_mul(3));
    for list in edges.iter().flatten() {
        if list.is_empty() {
            continue;
        }
        let (mut top, mut bottom) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut left, mut right) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut crossings: u64 = 0;
        for e in list {
            top = top.min(e.y0);
            bottom = bottom.max(e.y1);
            left = left.min(e.x0.min(e.x1));
            right = right.max(e.x0.max(e.x1));
            let rows = ((e.y1.min(rh as f64) - e.y0.max(0.0)).max(0.0) * scale).ceil() + 1.0;
            crossings = crossings.saturating_add(rows as u64);
        }
        let log = u64::from(64 - (list.len() as u64).leading_zeros());
        let box_w = (right.min(rw as f64) - left.max(0.0)).max(0.0).ceil() + 2.0;
        let box_h = (bottom.min(rh as f64) - top.max(0.0)).max(0.0).ceil() + 1.0;
        work = work
            .saturating_add(crossings.saturating_mul(1 + log))
            .saturating_add((box_h * scale) as u64)
            .saturating_add((2.0 * box_w * box_h) as u64);
    }
    work
}

/// Rasterises `path` over a `width × height` document, charging its
/// raster and its work against `budget`, all checked before any buffer
/// sized by them exists. The flattening charge — [`SEGMENT_CHARGE`] work
/// units per segment the path *will* flatten into, counted by `segment_total`
/// without allocating — is taken first and kept even when a later bound
/// refuses the mask, because flattening (and an off-canvas path's empty
/// raster) is done by then; the pixel and scanline charges are taken
/// only on success.
///
/// A group none of whose subpaths draws anything (fewer than two knots,
/// or no extent) has a zero plane everywhere, so its operation is the
/// same scalar map at every pixel: it is applied to a constant while the
/// mask is still uniform, and on a materialised mask it is the identity
/// (exclude, combine, a later subtract) or a reset to the constant `0`
/// (intersect). Either way it costs no per-pixel pass, so a flood of
/// empty subpath records cannot multiply the raster's cost.
#[allow(clippy::too_many_lines)] // one pass: flatten, bound, fill, combine
pub(super) fn rasterize(
    path: &VectorPath,
    width: u32,
    height: u32,
    budget: &mut Budget,
) -> Result<Raster, VectorFailure> {
    let (w, h) = (f64::from(width), f64::from(height));
    let planned = segment_total(path, w, h);
    let flattening = planned.saturating_mul(SEGMENT_CHARGE);
    if planned > MAX_FLATTENED_SEGMENTS as u64 || flattening > budget.work {
        return Err(VectorFailure::TooLarge);
    }
    budget.work -= flattening;
    let mut polygons: Vec<Option<Vec<(f64, f64)>>> = Vec::new();
    polygons
        .try_reserve_exact(path.subpaths.len())
        .map_err(|_| VectorFailure::TooLarge)?;
    for subpath in &path.subpaths {
        polygons.push(flatten(subpath, w, h)?);
    }
    let groups = groups(path);

    // The value outside every path.
    let initial = if path.initial_fill && path.subpaths.is_empty() {
        1.0
    } else {
        0.0
    };
    let mut outside = initial;
    for (index, (op, _)) in groups.iter().enumerate() {
        outside = combine(outside, 0.0, *op, index == 0);
    }
    let outside = finish(outside, path.invert) >= 0.5;

    // The raster rectangle: every point's bounding box, grown by a pixel
    // and clipped to the canvas.
    let (mut x0, mut y0, mut x1, mut y1) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for (x, y) in polygons.iter().flatten().flatten() {
        x0 = x0.min(*x);
        y0 = y0.min(*y);
        x1 = x1.max(*x);
        y1 = y1.max(*y);
    }
    let bounds = if x0 <= x1 && y0 <= y1 {
        let left = (x0.floor() - 1.0).clamp(0.0, w);
        let top = (y0.floor() - 1.0).clamp(0.0, h);
        let right = (x1.ceil() + 1.0).clamp(0.0, w);
        let bottom = (y1.ceil() + 1.0).clamp(0.0, h);
        Rect {
            x: left as i64,
            y: top as i64,
            width: (right - left).max(0.0) as u32,
            height: (bottom - top).max(0.0) as u32,
        }
    } else {
        Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    };
    let area = u64::from(bounds.width) * u64::from(bounds.height);
    let charged = area.saturating_mul(RASTER_PIXEL_CHARGE);
    if charged > budget.pixels {
        return Err(VectorFailure::TooLarge);
    }
    if area == 0 {
        return Ok(Raster {
            bounds,
            coverage: Vec::new(),
            outside,
            charged: 0,
        });
    }

    // Raster-local edges per polygon.
    let (ox, oy) = (bounds.x as f64, bounds.y as f64);
    let (rw, rh) = (bounds.width as usize, bounds.height as usize);
    let mut edges: Vec<Option<Vec<Edge>>> = Vec::new();
    edges
        .try_reserve_exact(polygons.len())
        .map_err(|_| VectorFailure::TooLarge)?;
    for polygon in &polygons {
        let Some(points) = polygon else {
            edges.push(None);
            continue;
        };
        let mut list = Vec::new();
        list.try_reserve_exact(points.len())
            .map_err(|_| VectorFailure::TooLarge)?;
        for (i, a) in points.iter().enumerate() {
            let Some(b) = points.get((i + 1) % points.len()) else {
                continue;
            };
            let (ax, ay, bx, by) = (a.0 - ox, a.1 - oy, b.0 - ox, b.1 - oy);
            #[allow(clippy::float_cmp)] // exactly horizontal: no crossing
            if ay == by {
                continue;
            }
            list.push(if ay < by {
                Edge {
                    x0: ax,
                    y0: ay,
                    x1: bx,
                    y1: by,
                    winding: 1,
                }
            } else {
                Edge {
                    x0: bx,
                    y0: by,
                    x1: ax,
                    y1: ay,
                    winding: -1,
                }
            });
        }
        edges.push(Some(list));
    }
    // Whether each group draws anything.
    let drawn: Vec<bool> = groups
        .iter()
        .map(|(_, members)| {
            members.iter().any(|member| {
                edges
                    .get(*member)
                    .is_some_and(|e| e.as_ref().is_some_and(|l| !l.is_empty()))
            })
        })
        .collect();
    let nonempty = drawn.iter().filter(|d| **d).count() as u64;
    let work = work_for(&edges, nonempty, rw, rh);
    if work > MAX_RASTER_WORK || work > budget.work {
        return Err(VectorFailure::TooLarge);
    }

    let len = rw.saturating_mul(rh);
    let mut mask: Vec<f32> = alloc(len, initial)?;
    let mut plane: Vec<f32> = alloc(len, 0.0)?;
    let mut direct: Vec<f32> = alloc(rw + 1, 0.0)?;
    let mut delta: Vec<f32> = alloc(rw + 2, 0.0)?;
    // `Some(c)`: the mask is `c` everywhere and the buffer is stale.
    let mut uniform = Some(initial);
    for (index, (op, members)) in groups.iter().enumerate() {
        let first = index == 0;
        if !drawn.get(index).copied().unwrap_or(false) {
            uniform = match uniform {
                Some(c) => Some(combine(c, 0.0, *op, first)),
                // Not uniform means a drawn group came first, so this one
                // is not `first`: intersect clears, the rest are identity.
                None if *op == 3 => Some(0.0),
                None => None,
            };
            continue;
        }
        if let Some(c) = uniform.take() {
            mask.fill(c);
        }
        plane.fill(0.0);
        for member in members {
            if let Some(Some(list)) = edges.get_mut(*member) {
                fill_nonzero(list, rw, rh, &mut plane, &mut direct, &mut delta);
            }
        }
        for (m, p) in mask.iter_mut().zip(&plane) {
            *m = combine(*m, *p, *op, first);
        }
    }
    let mut coverage = Vec::new();
    coverage
        .try_reserve_exact(len)
        .map_err(|_| VectorFailure::TooLarge)?;
    match uniform {
        Some(c) => coverage.resize(len, f16::from_f32(finish(c, path.invert))),
        None => coverage.extend(mask.iter().map(|m| f16::from_f32(finish(*m, path.invert)))),
    }
    budget.pixels -= charged;
    budget.work -= work;
    Ok(Raster {
        bounds,
        coverage,
        outside,
        charged,
    })
}

fn finish(m: f32, invert: bool) -> f32 {
    let m = m.clamp(0.0, 1.0);
    if invert { 1.0 - m } else { m }
}

fn alloc(len: usize, value: f32) -> Result<Vec<f32>, VectorFailure> {
    let mut out = Vec::new();
    out.try_reserve_exact(len)
        .map_err(|_| VectorFailure::TooLarge)?;
    out.resize(len, value);
    Ok(out)
}

/// Adds the span `[xa, xb)` (raster-local, any order-checked values) at
/// `weight` to one row's accumulators: exact fractional coverage at the
/// two end pixels, a running-sum step for the pixels between.
fn add_span(xa: f64, xb: f64, weight: f32, width: usize, direct: &mut [f32], delta: &mut [f32]) {
    let w = width as f64;
    let (xa, xb) = (xa.clamp(0.0, w), xb.clamp(0.0, w));
    if xb <= xa {
        return;
    }
    let ia = xa.floor() as usize;
    let ib = xb.floor() as usize;
    let add = |buf: &mut [f32], i: usize, v: f32| {
        if let Some(slot) = buf.get_mut(i) {
            *slot += v;
        }
    };
    if ia == ib {
        add(direct, ia, (xb - xa) as f32 * weight);
    } else {
        add(direct, ia, ((ia + 1) as f64 - xa) as f32 * weight);
        add(delta, ia + 1, weight);
        add(delta, ib, -weight);
        add(direct, ib, (xb - ib as f64) as f32 * weight);
    }
}

/// Fills one polygon (its edges) with the non-zero rule into `plane`,
/// composited "over" what is there.
fn fill_nonzero(
    edges: &mut [Edge],
    width: usize,
    height: usize,
    plane: &mut [f32],
    direct: &mut [f32],
    delta: &mut [f32],
) {
    if edges.is_empty() {
        return;
    }
    edges.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    let (mut top, mut bottom) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut left, mut right) = (f64::INFINITY, f64::NEG_INFINITY);
    for e in edges.iter() {
        top = top.min(e.y0);
        bottom = bottom.max(e.y1);
        left = left.min(e.x0.min(e.x1));
        right = right.max(e.x0.max(e.x1));
    }
    let first_row = top.floor().clamp(0.0, height as f64) as usize;
    let last_row = bottom.ceil().clamp(0.0, height as f64) as usize;
    let first_col = left.floor().clamp(0.0, width as f64) as usize;
    let last_col = (right.ceil() + 1.0).clamp(0.0, width as f64) as usize;
    let weight = 1.0 / SUBSAMPLES as f32;
    let mut next = 0;
    let mut active: Vec<Edge> = Vec::new();
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    for row in first_row..last_row {
        for s in 0..SUBSAMPLES {
            let y = row as f64 + (f64::from(s) + 0.5) / f64::from(SUBSAMPLES);
            while let Some(edge) = edges.get(next) {
                if edge.y0 > y {
                    break;
                }
                active.push(*edge);
                next += 1;
            }
            active.retain(|e| e.y1 > y);
            crossings.clear();
            for e in &active {
                if e.y0 <= y {
                    let x = e.x0 + (y - e.y0) * (e.x1 - e.x0) / (e.y1 - e.y0);
                    crossings.push((x, e.winding));
                }
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            let mut start = 0.0;
            for (x, dir) in &crossings {
                let before = winding;
                winding += dir;
                if before == 0 && winding != 0 {
                    start = *x;
                } else if before != 0 && winding == 0 {
                    add_span(start, *x, weight, width, direct, delta);
                }
            }
        }
        let mut running = 0.0_f32;
        let base = row * width;
        for col in first_col..last_col {
            running += delta.get(col).copied().unwrap_or(0.0);
            let c = (running + direct.get(col).copied().unwrap_or(0.0)).clamp(0.0, 1.0);
            if let Some(p) = plane.get_mut(base + col) {
                *p += (1.0 - *p) * c;
            }
        }
        let reset = first_col..(last_col + 2);
        if let Some(slots) = direct.get_mut(first_col..(last_col + 1).min(direct.len())) {
            slots.fill(0.0);
        }
        if let Some(slots) = delta.get_mut(first_col..reset.end.min(delta.len())) {
            slots.fill(0.0);
        }
    }
}
