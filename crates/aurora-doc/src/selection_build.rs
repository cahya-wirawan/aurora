//! Building raster selections (0.214.0): the Lasso tools' polygon becomes a
//! [`SelectionMask`] ([`polygon_mask`]) and the Magic Wand's matched pixels
//! become a mask with a softened outer edge ([`antialiased_mask`]).
//!
//! **Polygon rule: nonzero winding**, as Photoshop's Lasso fills: a point is
//! inside when the polygon's edges wind around it a non-zero number of
//! times, so a self-intersecting freehand loop fills every lobe, a region
//! covered twice in the same direction is filled once, and a region where
//! two lobes of opposite direction overlap is a hole (winding zero). The
//! polygon is closed implicitly (last vertex back to the first).
//!
//! **Coverage.** Each pixel row is sampled at [`SUBSCANLINES`] evenly spaced
//! heights; on each, the edge crossings are found in `f64`, the spans of
//! non-zero winding are integrated exactly across the pixels they touch
//! horizontally, and the row's coverage is the mean over the sub-scanlines.
//! A straight edge is therefore integrated exactly (the midpoint of a
//! linear function), and only the vertical resolution of a corner or a
//! curve is limited to quarter steps. The result is deterministic: no
//! randomness and no dependence on the order of the vertices beyond the
//! winding rule itself.
//!
//! **Bounded.** At most [`MAX_POLYGON_VERTICES`] vertices (non-finite ones
//! are dropped, coordinates are clamped to [`COORDINATE_LIMIT`]), the
//! area is clipped to the caller's `clip` rectangle first, and a clipped
//! area over [`crate::MAX_SELECTION_MASK_PIXELS`] is refused. The mask's
//! bounds are tight: the smallest rectangle holding every non-zero texel.

use aurora_core::Rect;

use crate::error::DocError;
use crate::selection_mask::{MAX_SELECTION_MASK_PIXELS, SelectionMask};

/// The most vertices a polygon may have.
pub const MAX_POLYGON_VERTICES: usize = 100_000;

/// Sub-scanlines sampled per pixel row.
pub const SUBSCANLINES: u32 = 4;

/// Vertex coordinates are clamped to `±COORDINATE_LIMIT` document pixels
/// (far beyond the 300,000 px document ceiling, small enough that every
/// slope and crossing stays finite in `f64`).
pub const COORDINATE_LIMIT: f64 = 1.0e9;

/// One polygon edge, oriented downward (`y0 < y1`), with the direction it
/// was drawn in (`+1` downward, `-1` upward).
#[derive(Clone, Copy)]
struct Edge {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    dir: i32,
}

impl Edge {
    fn x_at(&self, y: f64) -> f64 {
        self.x0 + (y - self.y0) * (self.x1 - self.x0) / (self.y1 - self.y0)
    }
}

fn clamp_coordinate(v: f64) -> f64 {
    v.clamp(-COORDINATE_LIMIT, COORDINATE_LIMIT)
}

/// `usize` from a `u32`.
fn us(v: u32) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

/// The edges of the closed polygon through `points`, horizontal ones left
/// out, sorted by their top.
fn polygon_edges(points: &[(f64, f64)]) -> Vec<Edge> {
    let clean: Vec<(f64, f64)> = points
        .iter()
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .map(|&(x, y)| (clamp_coordinate(x), clamp_coordinate(y)))
        .collect();
    let mut edges = Vec::with_capacity(clean.len());
    for (i, &(ax, ay)) in clean.iter().enumerate() {
        let (bx, by) = clean
            .get((i + 1) % clean.len())
            .copied()
            .unwrap_or((ax, ay));
        // Exactly horizontal edges cross no sub-scanline.
        if ay.to_bits() == by.to_bits() {
            continue;
        }
        edges.push(if ay < by {
            Edge {
                x0: ax,
                y0: ay,
                x1: bx,
                y1: by,
                dir: 1,
            }
        } else {
            Edge {
                x0: bx,
                y0: by,
                x1: ax,
                y1: ay,
                dir: -1,
            }
        });
    }
    edges.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    edges
}

/// The integer rectangle holding `points`, clipped to `clip`; `None` when
/// they do not meet or there are no finite points.
fn clipped_extent(points: &[(f64, f64)], clip: &Rect) -> Option<Rect> {
    let mut lo = (f64::INFINITY, f64::INFINITY);
    let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &(x, y) in points
        .iter()
        .filter(|(x, y)| x.is_finite() && y.is_finite())
    {
        let (x, y) = (clamp_coordinate(x), clamp_coordinate(y));
        lo = (lo.0.min(x), lo.1.min(y));
        hi = (hi.0.max(x), hi.1.max(y));
    }
    if lo.0 > hi.0 {
        return None;
    }
    // Within ±1e9: exact in i64.
    #[allow(clippy::cast_possible_truncation)]
    let (x0, y0, x1, y1) = (
        lo.0.floor() as i64,
        lo.1.floor() as i64,
        hi.0.ceil() as i64,
        hi.1.ceil() as i64,
    );
    let (x0, y0) = (x0.max(clip.x), y0.max(clip.y));
    let (x1, y1) = (x1.min(clip.right()), y1.min(clip.bottom()));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(Rect {
        x: x0,
        y: y0,
        width: u32::try_from(x1 - x0).ok()?,
        height: u32::try_from(y1 - y0).ok()?,
    })
}

/// Adds the horizontal span `[a, b)` (local coordinates, `0..=width`) to
/// `acc` (partial pixels) and `diff` (whole pixels, as a difference array).
fn add_span(acc: &mut [f64], diff: &mut [i32], a: f64, b: f64) {
    let width = acc.len();
    #[allow(clippy::cast_precision_loss)]
    let wf = width as f64;
    let (a, b) = (a.clamp(0.0, wf), b.clamp(0.0, wf));
    if b <= a {
        return;
    }
    // In 0..=width after the clamp.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (ia, ib) = (a.floor() as usize, b.floor() as usize);
    #[allow(clippy::cast_precision_loss)]
    let (fa, fb) = (ia as f64, ib as f64);
    if ia == ib {
        if let Some(cell) = acc.get_mut(ia) {
            *cell += b - a;
        }
        return;
    }
    if let Some(cell) = acc.get_mut(ia) {
        *cell += fa + 1.0 - a;
    }
    if let Some(cell) = acc.get_mut(ib) {
        *cell += b - fb;
    }
    if ib > ia + 1 {
        if let Some(d) = diff.get_mut(ia + 1) {
            *d += 1;
        }
        if let Some(d) = diff.get_mut(ib) {
            *d -= 1;
        }
    }
}

/// The antialiased [`SelectionMask`] of the polygon through `points`
/// (document pixels, `(x, y)`; pixel `(i, j)` covers `[i, i+1) x [j, j+1)`),
/// clipped to `clip`, by the nonzero winding rule. `Ok(None)` when nothing
/// is covered (fewer than three usable vertices, a degenerate polygon, or
/// one wholly outside `clip`): the caller deselects.
///
/// # Errors
///
/// [`DocError::SelectionPolygonTooComplex`] over [`MAX_POLYGON_VERTICES`]
/// vertices; [`DocError::SelectionMaskTooLarge`] when the clipped area is
/// over [`MAX_SELECTION_MASK_PIXELS`].
pub fn polygon_mask(points: &[(f64, f64)], clip: &Rect) -> Result<Option<SelectionMask>, DocError> {
    if points.len() > MAX_POLYGON_VERTICES {
        return Err(DocError::SelectionPolygonTooComplex(points.len()));
    }
    let edges = polygon_edges(points);
    if edges.len() < 2 {
        return Ok(None);
    }
    let Some(area) = clipped_extent(points, clip) else {
        return Ok(None);
    };
    let pixels = area.area_px();
    if pixels > MAX_SELECTION_MASK_PIXELS {
        return Err(DocError::SelectionMaskTooLarge(pixels));
    }
    let (w, h) = (us(area.width), us(area.height));
    let mut data = vec![0_u8; w.saturating_mul(h)];
    let mut acc = vec![0.0_f64; w];
    let mut diff = vec![0_i32; w + 1];
    let mut active: Vec<Edge> = Vec::new();
    let mut next_edge = 0_usize;
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    let sub = f64::from(SUBSCANLINES);
    #[allow(clippy::cast_precision_loss)]
    let origin_x = area.x as f64;
    // Tight bounds of the non-zero texels, local.
    let mut tight: Option<(usize, usize, usize, usize)> = None;
    for (row, out_row) in data.chunks_exact_mut(w).enumerate() {
        acc.fill(0.0);
        diff.fill(0);
        for k in 0..SUBSCANLINES {
            #[allow(clippy::cast_precision_loss)]
            let yc = area.y as f64 + row as f64 + (f64::from(k) + 0.5) / sub;
            while let Some(edge) = edges.get(next_edge)
                && edge.y0 <= yc
            {
                active.push(*edge);
                next_edge += 1;
            }
            active.retain(|e| e.y1 > yc);
            crossings.clear();
            crossings.extend(active.iter().map(|e| (e.x_at(yc), e.dir)));
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0_i32;
            let mut previous = 0.0_f64;
            for &(x, dir) in &crossings {
                if winding != 0 {
                    add_span(&mut acc, &mut diff, previous - origin_x, x - origin_x);
                }
                winding += dir;
                previous = x;
            }
        }
        let mut run = 0_i32;
        for (x, (cell, d)) in acc.iter().zip(diff.iter()).enumerate() {
            run += d;
            let coverage = ((cell + f64::from(run)) / sub).clamp(0.0, 1.0);
            // In 0..=255 after the clamp.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let byte = (coverage * 255.0).round() as u8;
            if byte != 0 {
                if let Some(slot) = out_row.get_mut(x) {
                    *slot = byte;
                }
                tight = Some(match tight {
                    None => (x, row, x, row),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(row), x1.max(x), y1.max(row)),
                });
            }
        }
    }
    let Some((x0, y0, x1, y1)) = tight else {
        return Ok(None);
    };
    crop_mask(&area, &data, (x0, y0, x1, y1)).map(Some)
}

/// The mask over the tight rectangle `(x0, y0, x1, y1)` (inclusive, local
/// to `area`) of `data`, which covers `area`.
fn crop_mask(
    area: &Rect,
    data: &[u8],
    (x0, y0, x1, y1): (usize, usize, usize, usize),
) -> Result<SelectionMask, DocError> {
    let w = us(area.width);
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = Vec::with_capacity(cw.saturating_mul(ch));
    for y in y0..=y1 {
        if let Some(row) = data.get(y * w + x0..y * w + x0 + cw) {
            out.extend_from_slice(row);
        }
    }
    let bounds = Rect {
        x: area.x + i64::try_from(x0).map_err(|_| DocError::SelectionMaskInvalid)?,
        y: area.y + i64::try_from(y0).map_err(|_| DocError::SelectionMaskInvalid)?,
        width: u32::try_from(cw).map_err(|_| DocError::SelectionMaskInvalid)?,
        height: u32::try_from(ch).map_err(|_| DocError::SelectionMaskInvalid)?,
    };
    SelectionMask::new(bounds, out)
}

/// A softened mask from a binary one (the Magic Wand's "anti-alias"):
/// `data` is one byte per texel of `bounds`, non-zero = selected. Every
/// selected texel keeps full coverage (so a one-pixel region is still
/// fully selected); the texels around the region get a partial coverage
/// from a separable `1 2 1` blur of the selection (a straight edge's
/// outer neighbour a quarter, a corner's diagonal a sixteenth), which grows
/// the selection by at most one texel on every side without moving the
/// coverage-one-half outline. The result is cropped to the tight bounds
/// of its non-zero texels; `Ok(None)` for no selected texel at all.
///
/// # Errors
///
/// [`DocError::SelectionMaskInvalid`] when `data` is not one byte per
/// texel of a non-empty `bounds`; [`DocError::SelectionMaskTooLarge`]
/// when `bounds` grown by one texel is over the mask cap.
pub fn antialiased_mask(bounds: Rect, data: &[u8]) -> Result<Option<SelectionMask>, DocError> {
    if bounds.is_empty() || u64::try_from(data.len()).ok() != Some(bounds.area_px()) {
        return Err(DocError::SelectionMaskInvalid);
    }
    let area = Rect {
        x: bounds.x.saturating_sub(1),
        y: bounds.y.saturating_sub(1),
        width: bounds.width.saturating_add(2),
        height: bounds.height.saturating_add(2),
    };
    let pixels = area.area_px();
    if pixels > MAX_SELECTION_MASK_PIXELS {
        return Err(DocError::SelectionMaskTooLarge(pixels));
    }
    let (w, h) = (us(area.width), us(area.height));
    let (bw, bh) = (us(bounds.width), us(bounds.height));
    // Horizontal pass: weights 1 2 1 (sum 4) of the 0/1 selection.
    let selected = |x: usize, y: usize| -> u32 {
        // Local to `area`; `area` is `bounds` shifted by one texel.
        if x == 0 || y == 0 || x > bw || y > bh {
            return 0;
        }
        u32::from(data.get((y - 1) * bw + (x - 1)).is_some_and(|&v| v != 0))
    };
    let mut horizontal = vec![0_u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let left = if x == 0 { 0 } else { selected(x - 1, y) };
            let v = left + 2 * selected(x, y) + selected(x + 1, y);
            if let Some(slot) = horizontal.get_mut(y * w + x) {
                *slot = u8::try_from(v).unwrap_or(4);
            }
        }
    }
    let mut out = vec![0_u8; w * h];
    let mut tight: Option<(usize, usize, usize, usize)> = None;
    for y in 0..h {
        for x in 0..w {
            let at = |yy: Option<usize>| -> u32 {
                yy.and_then(|yy| horizontal.get(yy * w + x))
                    .map_or(0, |&v| u32::from(v))
            };
            let up = at(y.checked_sub(1));
            let here = at(Some(y));
            let down = at(y.checked_add(1).filter(|v| *v < h));
            // Sum is at most 16; 255 * sum / 16 rounded.
            let sum = up + 2 * here + down;
            let blurred = u8::try_from((sum * 255 + 8) / 16).unwrap_or(u8::MAX);
            // A selected texel keeps full coverage: only the outside edge
            // is softened, so even a one-pixel region stays selected.
            let byte = if selected(x, y) == 1 {
                u8::MAX
            } else {
                blurred
            };
            if byte != 0 {
                if let Some(slot) = out.get_mut(y * w + x) {
                    *slot = byte;
                }
                tight = Some(match tight {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    let Some(rect) = tight else {
        return Ok(None);
    };
    crop_mask(&area, &out, rect).map(Some)
}
