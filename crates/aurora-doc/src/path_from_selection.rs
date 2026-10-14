//! Make Work Path from Selection (0.234.0): the selection's outline as
//! Bezier subpaths.
//!
//! A rectangle gives its exact four corners, an ellipse the standard
//! four-anchor Bezier circle (kappa 0.5523) inscribed in its bounds, a
//! raster mask its `coverage >= 0.5` boundary (the same threshold the
//! marching ants use): pixel-edge loops are traced, thinned to corners and
//! simplified with Douglas-Peucker. Outer boundaries wind one way and
//! holes the other, so the nonzero rule of [`crate::selection_from_path`]
//! gives the region back. An inverted selection is the document rectangle
//! plus the reversed loops (holes).

use aurora_core::Rect;

use crate::path::{
    Anchor, MAX_ANCHORS_PER_SUBPATH, MAX_PATH_ANCHORS_TOTAL, MAX_SUBPATHS, Path, Subpath,
    WORK_PATH_NAME,
};
use crate::path_edit::reversed;
use crate::selection::{Selection, SelectionShape};
use crate::selection_mask::SelectionMask;

/// The Bezier circle constant.
const KAPPA: f32 = 0.552_284_8;
/// The first simplification tolerance in pixels; it doubles on each retry.
/// The traced corners of a raster edge already jitter up to half a pixel
/// from the true edge, so 0.5 would follow the jitter (1,010 anchors for a
/// 28-point star, `IoU` 0.99996); 1.0 gives 172 anchors and `IoU` 0.993.
pub const SIMPLIFY_TOLERANCE: f64 = 1.0;
/// How many tolerances are tried (1, 2, 4, 8).
pub const SIMPLIFY_TRIES: u32 = 4;
/// The most pixel-edges a mask boundary may have before the trace gives up.
pub const MAX_TRACE_EDGES: usize = 4_000_000;

/// Why a selection could not become a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathFromSelectionError {
    /// The selection covers nothing.
    Empty,
    /// The outline is more complex than a path may be, even simplified.
    TooComplex,
}

fn rect_subpath(r: &Rect) -> Subpath {
    #[allow(clippy::cast_precision_loss)]
    let (x0, y0, x1, y1) = (r.x as f32, r.y as f32, r.right() as f32, r.bottom() as f32);
    Subpath {
        anchors: vec![
            Anchor::corner(x0, y0),
            Anchor::corner(x1, y0),
            Anchor::corner(x1, y1),
            Anchor::corner(x0, y1),
        ],
        closed: true,
    }
}

fn ellipse_subpath(r: &Rect) -> Subpath {
    #[allow(clippy::cast_precision_loss)]
    let (a, b) = (r.width as f32 / 2.0, r.height as f32 / 2.0);
    #[allow(clippy::cast_precision_loss)]
    let (cx, cy) = (r.x as f32 + a, r.y as f32 + b);
    let (ka, kb) = (a * KAPPA, b * KAPPA);
    let anchor = |point: (f32, f32), out: (f32, f32)| Anchor {
        point,
        handle_in: (-out.0, -out.1),
        handle_out: out,
    };
    Subpath {
        // Right, bottom, left, top: clockwise on screen, like a rectangle.
        anchors: vec![
            anchor((cx + a, cy), (0.0, kb)),
            anchor((cx, cy + b), (-ka, 0.0)),
            anchor((cx - a, cy), (0.0, -kb)),
            anchor((cx, cy - b), (ka, 0.0)),
        ],
        closed: true,
    }
}

type P64 = (i64, i64);

/// 0 right, 1 down, 2 left, 3 up (screen coordinates, y down).
const DELTA: [P64; 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];

/// The pixel-edge loops of the `coverage >= 128` region, as corner lists in
/// document pixels. Outer boundaries run clockwise on screen (positive
/// shoelace sum), holes the other way.
fn trace_loops(mask: &SelectionMask) -> Option<Vec<Vec<P64>>> {
    let b = mask.bounds();
    let inside = |x: i64, y: i64| mask.byte_at(x, y) >= 128 && b.contains_point(x, y);
    let mut edges: std::collections::HashMap<P64, [bool; 4]> = std::collections::HashMap::new();
    let mut count = 0_usize;
    let mut add = |from: P64, dir: usize| -> bool {
        count += 1;
        if let Some(slot) = edges.entry(from).or_default().get_mut(dir) {
            *slot = true;
        }
        count <= MAX_TRACE_EDGES
    };
    for y in b.y..b.bottom() {
        for x in b.x..b.right() {
            if !inside(x, y) {
                continue;
            }
            let ok = (inside(x, y - 1) || add((x, y), 0))
                && (inside(x + 1, y) || add((x + 1, y), 1))
                && (inside(x, y + 1) || add((x + 1, y + 1), 2))
                && (inside(x - 1, y) || add((x, y + 1), 3));
            if !ok {
                return None;
            }
        }
    }
    let mut starts: Vec<P64> = edges.keys().copied().collect();
    starts.sort_unstable();
    let mut loops = Vec::new();
    for start in starts {
        while let Some(first) = edges
            .get(&start)
            .and_then(|slots| slots.iter().position(|&present| present))
        {
            let mut corners = Vec::new();
            let mut cur = start;
            let mut dir = first;
            let mut prev_dir = usize::MAX;
            loop {
                if let Some(slot) = edges.get_mut(&cur).and_then(|s| s.get_mut(dir)) {
                    *slot = false;
                }
                if dir != prev_dir {
                    corners.push(cur);
                }
                prev_dir = dir;
                let (dx, dy) = DELTA.get(dir).copied().unwrap_or((0, 0));
                cur = (cur.0 + dx, cur.1 + dy);
                if cur == start {
                    break;
                }
                // Prefer the right turn, then straight, then left: diagonal
                // neighbours stay separate loops.
                let Some(slots) = edges.get(&cur) else {
                    break;
                };
                let Some(next) = [(dir + 1) % 4, dir, (dir + 3) % 4]
                    .into_iter()
                    .find(|d| slots.get(*d).copied().unwrap_or(false))
                else {
                    break;
                };
                dir = next;
            }
            // The first corner is the start vertex only when the loop
            // turned there; a straight run through it is merged.
            if corners.len() >= 2 && prev_dir == first && corners.first() == Some(&start) {
                corners.remove(0);
            }
            if corners.len() >= 3 {
                loops.push(corners);
            }
        }
    }
    Some(loops)
}

fn signed_area2(points: &[(f64, f64)]) -> f64 {
    let mut sum = 0.0;
    for (i, p) in points.iter().enumerate() {
        if let Some(q) = points.get((i + 1) % points.len()) {
            sum += p.0 * q.1 - q.0 * p.1;
        }
    }
    sum
}

fn point_line_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy);
    if len <= 0.0 {
        return (p.0 - a.0).hypot(p.1 - a.1);
    }
    ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / len
}

/// Douglas-Peucker on the open run `points[lo..=hi]`, marking kept indices.
fn dp_run(points: &[(f64, f64)], lo: usize, hi: usize, tol: f64, keep: &mut [bool]) {
    let mut stack = vec![(lo, hi)];
    while let Some((l, h)) = stack.pop() {
        let (Some(&a), Some(&b)) = (points.get(l), points.get(h % points.len())) else {
            continue;
        };
        let mut far = (0.0_f64, l);
        for i in l + 1..h {
            if let Some(&p) = points.get(i % points.len()) {
                let d = point_line_distance(p, a, b);
                if d > far.0 {
                    far = (d, i);
                }
            }
        }
        if far.0 > tol {
            if let Some(k) = keep.get_mut(far.1 % points.len()) {
                *k = true;
            }
            stack.push((l, far.1));
            stack.push((far.1, h));
        }
    }
}

/// Douglas-Peucker for a closed loop: split at the vertex farthest from
/// the first and simplify both halves.
fn simplify_closed(points: &[(f64, f64)], tol: f64) -> Vec<(f64, f64)> {
    let n = points.len();
    if n <= 3 {
        return points.to_vec();
    }
    let Some(&origin) = points.first() else {
        return Vec::new();
    };
    let split = points
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            let da = (a.0 - origin.0).hypot(a.1 - origin.1);
            let db = (b.0 - origin.0).hypot(b.1 - origin.1);
            da.total_cmp(&db)
        })
        .map_or(0, |(i, _)| i);
    let mut keep = vec![false; n];
    if let Some(k) = keep.first_mut() {
        *k = true;
    }
    if let Some(k) = keep.get_mut(split) {
        *k = true;
    }
    dp_run(points, 0, split, tol, &mut keep);
    dp_run(points, split, n, tol, &mut keep);
    points
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

fn polygon_subpath(points: &[(f64, f64)]) -> Subpath {
    Subpath {
        anchors: points
            .iter()
            .map(|&(x, y)| {
                #[allow(clippy::cast_possible_truncation)]
                Anchor::corner(x as f32, y as f32)
            })
            .collect(),
        closed: true,
    }
}

/// The simplified subpaths of a traced mask at one tolerance.
fn simplified(loops: &[Vec<P64>], tol: f64) -> Vec<Subpath> {
    loops
        .iter()
        .filter_map(|corners| {
            #[allow(clippy::cast_precision_loss)]
            let pts: Vec<(f64, f64)> = corners.iter().map(|&(x, y)| (x as f64, y as f64)).collect();
            let mut kept = simplify_closed(&pts, tol);
            if kept.len() < 3 {
                // A tiny loop that would collapse keeps its corners.
                kept = pts;
            }
            // A sliver is dropped, and so is a speck smaller than the
            // tolerance squared once the tolerance is coarser than a pixel.
            let least = if tol > 1.0 { tol * tol } else { 1.0 };
            (kept.len() >= 3 && signed_area2(&kept).abs() / 2.0 >= least - 1e-9)
                .then(|| polygon_subpath(&kept))
        })
        .collect()
}

fn within_caps(subs: &[Subpath]) -> bool {
    subs.len() <= MAX_SUBPATHS
        && subs
            .iter()
            .all(|s| s.anchors.len() <= MAX_ANCHORS_PER_SUBPATH)
        && subs.iter().map(|s| s.anchors.len()).sum::<usize>() <= MAX_PATH_ANCHORS_TOTAL
}

/// The work path outlining `selection` in a document of `size`.
///
/// # Errors
///
/// [`PathFromSelectionError::Empty`] for a selection that covers nothing;
/// [`PathFromSelectionError::TooComplex`] when even the coarsest
/// simplification ([`SIMPLIFY_TRIES`] tolerances from [`SIMPLIFY_TOLERANCE`])
/// exceeds the path caps or the trace is too long.
pub fn path_from_selection(
    selection: &Selection,
    size: (u32, u32),
) -> Result<Path, PathFromSelectionError> {
    let shape_subs: Vec<Subpath> = match &selection.shape {
        SelectionShape::Rect => {
            if selection.bounds.width == 0 || selection.bounds.height == 0 {
                return Err(PathFromSelectionError::Empty);
            }
            vec![rect_subpath(&selection.bounds)]
        }
        SelectionShape::Ellipse => {
            if selection.bounds.width == 0 || selection.bounds.height == 0 {
                return Err(PathFromSelectionError::Empty);
            }
            vec![ellipse_subpath(&selection.bounds)]
        }
        SelectionShape::Mask(mask) => {
            let loops = trace_loops(mask).ok_or(PathFromSelectionError::TooComplex)?;
            if loops.is_empty() {
                if selection.inverted {
                    Vec::new()
                } else {
                    return Err(PathFromSelectionError::Empty);
                }
            } else {
                let mut tol = SIMPLIFY_TOLERANCE;
                let mut found = None;
                for _ in 0..SIMPLIFY_TRIES {
                    let subs = simplified(&loops, tol);
                    if !subs.is_empty() && within_caps(&subs) {
                        found = Some(subs);
                        break;
                    }
                    tol *= 2.0;
                }
                found.ok_or(PathFromSelectionError::TooComplex)?
            }
        }
    };
    let subpaths = if selection.inverted {
        let document = Rect {
            x: 0,
            y: 0,
            width: size.0,
            height: size.1,
        };
        let mut subs = vec![rect_subpath(&document)];
        subs.extend(shape_subs.iter().map(reversed));
        subs
    } else {
        shape_subs
    };
    if !within_caps(&subpaths) {
        return Err(PathFromSelectionError::TooComplex);
    }
    Ok(Path {
        name: WORK_PATH_NAME.to_owned(),
        subpaths,
    })
}

#[cfg(test)]
#[path = "path_from_selection_tests.rs"]
mod tests;
