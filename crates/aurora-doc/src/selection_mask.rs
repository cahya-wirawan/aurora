//! The raster half of a selection (0.213.0): a bounded 8-bit coverage grid
//! over a document-space rectangle, plus the cached boundary trace the
//! marching-ants outline is drawn from.
//!
//! A [`SelectionMask`] holds one byte per texel of its `bounds` (`0` =
//! unselected, `255` = fully selected, row-major); outside `bounds` the
//! coverage is `0`. It is immutable once built and shared through an
//! [`std::sync::Arc`], so undo history and the painting adapter hold it
//! for the price of a pointer. Region queries (`fully_covers`,
//! `fully_excludes`) are answered from 16 x 16 block minima and maxima
//! built at construction, so a brush asking about a 64 x 64 tile does not
//! scan 4096 texels.
//!
//! **Bounded:** at most [`MAX_SELECTION_MASK_PIXELS`] texels (64 Mpx, 64 MiB);
//! a larger request is refused with [`DocError::SelectionMaskTooLarge`].
//! The boundary trace holds at most [`MAX_TRACE_RUNS`] runs; a pathological
//! mask (a checkerboard) is traced only partly and says so
//! ([`BoundaryTrace::truncated`]).

use std::sync::OnceLock;

use aurora_core::Rect;

use crate::error::DocError;

/// The most texels a mask may cover (64 Mpx, so at most 64 MiB of bytes).
pub const MAX_SELECTION_MASK_PIXELS: u64 = 64 * 1024 * 1024;

/// The most runs a boundary trace keeps.
pub const MAX_TRACE_RUNS: usize = 262_144;

/// The edge length of a summary block.
const BLOCK: usize = 16;

/// A texel is "inside" for the outline when its coverage is at least
/// one half: `128 / 255` is the first byte past `0.5`.
const INSIDE_THRESHOLD: u8 = 128;

/// One straight stretch of the selection's boundary, in document space:
/// `len` unit edges starting at `(x, y)`, running right (`vertical` false:
/// a horizontal edge along the top of texels `x..x+len` in row `y`) or
/// down (`vertical` true: along the left of texels `y..y+len` in column
/// `x`). `base` is the number of boundary edges before it in the trace,
/// which the outline uses to number its dashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceRun {
    pub x: i64,
    pub y: i64,
    pub len: u32,
    pub vertical: bool,
    /// Whether the covered side (coverage at least one half) lies after
    /// the edge (below a horizontal one, right of a vertical one). The
    /// outline draws an edge with the covered side before it on the
    /// pixel just inside, like a rectangle's right and bottom border.
    pub covered_after: bool,
    pub base: u64,
}

/// The boundary of a mask at the coverage-0.5 threshold.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BoundaryTrace {
    pub runs: Vec<TraceRun>,
    /// Whether the mask had more runs than [`MAX_TRACE_RUNS`] and the
    /// trace stops short.
    pub truncated: bool,
}

/// A bounded 8-bit coverage grid over `bounds`.
pub struct SelectionMask {
    bounds: Rect,
    data: Vec<u8>,
    /// `(min, max)` per `BLOCK x BLOCK` block, row-major.
    blocks: Vec<(u8, u8)>,
    blocks_across: usize,
    trace: OnceLock<BoundaryTrace>,
}

impl std::fmt::Debug for SelectionMask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectionMask")
            .field("bounds", &self.bounds)
            .field("texels", &self.data.len())
            .finish_non_exhaustive()
    }
}

impl PartialEq for SelectionMask {
    fn eq(&self, other: &Self) -> bool {
        self.bounds == other.bounds && self.data == other.data
    }
}

impl Eq for SelectionMask {}

/// `usize` from a `u32` (every supported target has at least 32 bits).
fn us(v: u32) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

impl SelectionMask {
    /// A mask over `bounds` with one coverage byte per texel, row-major.
    ///
    /// # Errors
    ///
    /// [`DocError::SelectionMaskTooLarge`] if `bounds` has more than
    /// [`MAX_SELECTION_MASK_PIXELS`] texels, [`DocError::SelectionMaskInvalid`]
    /// if `bounds` is empty or `data` is not exactly one byte per texel.
    pub fn new(bounds: Rect, data: Vec<u8>) -> Result<Self, DocError> {
        let pixels = bounds.area_px();
        if pixels > MAX_SELECTION_MASK_PIXELS {
            return Err(DocError::SelectionMaskTooLarge(pixels));
        }
        if bounds.is_empty() || u64::try_from(data.len()).ok() != Some(pixels) {
            return Err(DocError::SelectionMaskInvalid);
        }
        let (w, h) = (us(bounds.width), us(bounds.height));
        let blocks_across = w.div_ceil(BLOCK);
        let blocks_down = h.div_ceil(BLOCK);
        let mut blocks = vec![(u8::MAX, u8::MIN); blocks_across * blocks_down];
        for (ly, row) in data.chunks_exact(w).enumerate() {
            for (bx, chunk) in row.chunks(BLOCK).enumerate() {
                let (lo, hi) = chunk
                    .iter()
                    .fold((u8::MAX, u8::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
                if let Some(cell) = blocks.get_mut((ly / BLOCK) * blocks_across + bx) {
                    cell.0 = cell.0.min(lo);
                    cell.1 = cell.1.max(hi);
                }
            }
        }
        Ok(Self {
            bounds,
            data,
            blocks,
            blocks_across,
            trace: OnceLock::new(),
        })
    }

    /// Builds a mask over `bounds` from `coverage(x, y)`, called once per
    /// document texel and clamped to `0..=1`, quantised to the nearest
    /// byte.
    ///
    /// # Errors
    ///
    /// The errors of [`Self::new`].
    pub fn rasterize(bounds: Rect, coverage: impl Fn(i64, i64) -> f32) -> Result<Self, DocError> {
        let pixels = bounds.area_px();
        if pixels > MAX_SELECTION_MASK_PIXELS {
            return Err(DocError::SelectionMaskTooLarge(pixels));
        }
        let mut data = Vec::with_capacity(usize::try_from(pixels).unwrap_or(0));
        for y in bounds.y..bounds.bottom() {
            for x in bounds.x..bounds.right() {
                let c = coverage(x, y);
                let c = if c.is_finite() {
                    c.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                // In 0..=255 after the clamp.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                data.push((c * 255.0).round() as u8);
            }
        }
        Self::new(bounds, data)
    }

    /// The document rectangle the grid covers.
    #[must_use]
    pub const fn bounds(&self) -> Rect {
        self.bounds
    }

    /// The coverage byte of document texel `(x, y)`; `0` outside the grid.
    #[must_use]
    pub fn byte_at(&self, x: i64, y: i64) -> u8 {
        if !self.bounds.contains_point(x, y) {
            return 0;
        }
        let (lx, ly) = (x - self.bounds.x, y - self.bounds.y);
        let (Ok(lx), Ok(ly)) = (usize::try_from(lx), usize::try_from(ly)) else {
            return 0;
        };
        self.data
            .get(ly * us(self.bounds.width) + lx)
            .copied()
            .unwrap_or(0)
    }

    /// The coverage of texel `(x, y)` in `0..=1`.
    #[must_use]
    pub fn coverage_at(&self, x: i64, y: i64) -> f32 {
        f32::from(self.byte_at(x, y)) / 255.0
    }

    /// The `(min, max)` coverage byte over `rect ∩ bounds`, or `None`
    /// when they do not meet.
    fn min_max(&self, rect: &Rect) -> Option<(u8, u8)> {
        if !self.bounds.intersects(rect) {
            return None;
        }
        let w = us(self.bounds.width);
        // Intersection in local texel coordinates (non-negative, in range).
        let lx0 = usize::try_from(rect.x.max(self.bounds.x) - self.bounds.x).ok()?;
        let ly0 = usize::try_from(rect.y.max(self.bounds.y) - self.bounds.y).ok()?;
        let lx1 = usize::try_from(rect.right().min(self.bounds.right()) - self.bounds.x).ok()?;
        let ly1 = usize::try_from(rect.bottom().min(self.bounds.bottom()) - self.bounds.y).ok()?;
        let (mut lo, mut hi) = (u8::MAX, u8::MIN);
        let mut take = |a: u8, b: u8| {
            lo = lo.min(a);
            hi = hi.max(b);
        };
        for by in (ly0 / BLOCK)..ly1.div_ceil(BLOCK) {
            for bx in (lx0 / BLOCK)..lx1.div_ceil(BLOCK) {
                let (bx0, by0) = (bx * BLOCK, by * BLOCK);
                let whole = lx0 <= bx0
                    && ly0 <= by0
                    && lx1 >= (bx0 + BLOCK).min(w)
                    && ly1 >= (by0 + BLOCK).min(us(self.bounds.height));
                if whole {
                    if let Some(&(a, b)) = self.blocks.get(by * self.blocks_across + bx) {
                        take(a, b);
                    }
                } else {
                    let (x0, x1) = (lx0.max(bx0), lx1.min(bx0 + BLOCK));
                    for ly in ly0.max(by0)..ly1.min(by0 + BLOCK) {
                        if let Some(row) = self.data.get(ly * w + x0..ly * w + x1) {
                            for &v in row {
                                take(v, v);
                            }
                        }
                    }
                }
            }
        }
        (lo <= hi).then_some((lo, hi))
    }

    /// Whether every texel of `rect` has coverage `255` (a `rect` reaching
    /// outside the grid is not covered there).
    #[must_use]
    pub fn fully_covers(&self, rect: &Rect) -> bool {
        let inside = self.bounds.x <= rect.x
            && self.bounds.y <= rect.y
            && self.bounds.right() >= rect.right()
            && self.bounds.bottom() >= rect.bottom();
        inside && self.min_max(rect).is_some_and(|(lo, _)| lo == u8::MAX)
    }

    /// Whether every texel of `rect` has coverage `0`.
    #[must_use]
    pub fn fully_excludes(&self, rect: &Rect) -> bool {
        self.min_max(rect).is_none_or(|(_, hi)| hi == 0)
    }

    /// The boundary of the region whose coverage is at least one half,
    /// traced once and cached for the life of the mask (so every frame
    /// reuses it): cost `O(texels)` the first time, at most
    /// [`MAX_TRACE_RUNS`] runs kept.
    #[must_use]
    pub fn boundary(&self) -> &BoundaryTrace {
        self.trace.get_or_init(|| self.trace_boundary())
    }

    fn trace_boundary(&self) -> BoundaryTrace {
        let (w, h) = (us(self.bounds.width), us(self.bounds.height));
        let inside = |row: Option<&[u8]>, x: usize| -> bool {
            row.and_then(|r| r.get(x))
                .is_some_and(|&v| v >= INSIDE_THRESHOLD)
        };
        let row_at = |y: usize| -> Option<&[u8]> { self.data.get(y * w..(y + 1) * w) };
        let mut out = BoundaryTrace::default();
        let mut base = 0_u64;
        let mut push = |out: &mut BoundaryTrace,
                        x: usize,
                        y: usize,
                        len: usize,
                        vertical: bool,
                        covered_after: bool| {
            if out.runs.len() >= MAX_TRACE_RUNS {
                out.truncated = true;
                return;
            }
            let len32 = u32::try_from(len).unwrap_or(u32::MAX);
            out.runs.push(TraceRun {
                x: self.bounds.x.saturating_add(i64::try_from(x).unwrap_or(0)),
                y: self.bounds.y.saturating_add(i64::try_from(y).unwrap_or(0)),
                len: len32,
                vertical,
                covered_after,
                base,
            });
            base += u64::from(len32);
        };
        // Horizontal edges: between row `ey - 1` and row `ey`.
        for ey in 0..=h {
            let above = if ey == 0 { None } else { row_at(ey - 1) };
            let below = if ey == h { None } else { row_at(ey) };
            // The open stretch: where it began and which side it covers.
            let mut start: Option<(usize, bool)> = None;
            for x in 0..=w {
                let edge =
                    (x < w && inside(above, x) != inside(below, x)).then(|| inside(below, x));
                match (edge, start) {
                    (Some(side), None) => start = Some((x, side)),
                    (Some(side), Some((s, open))) if side != open => {
                        push(&mut out, s, ey, x - s, false, open);
                        start = Some((x, side));
                    }
                    (None, Some((s, open))) => {
                        push(&mut out, s, ey, x - s, false, open);
                        start = None;
                    }
                    _ => {}
                }
            }
        }
        // Vertical edges: between column `ex - 1` and column `ex`, merged
        // down the rows by remembering where each column's run opened.
        let mut open: Vec<Option<(usize, bool)>> = vec![None; w + 1];
        for y in 0..=h {
            let row = if y < h { row_at(y) } else { None };
            for (ex, slot) in open.iter_mut().enumerate() {
                let left = ex > 0 && inside(row, ex - 1);
                let right = ex < w && inside(row, ex);
                let edge = (row.is_some() && left != right).then_some(right);
                match (edge, *slot) {
                    (Some(side), None) => *slot = Some((y, side)),
                    (Some(side), Some((s, open))) if side != open => {
                        push(&mut out, ex, s, y - s, true, open);
                        *slot = Some((y, side));
                    }
                    (None, Some((s, open))) => {
                        push(&mut out, ex, s, y - s, true, open);
                        *slot = None;
                    }
                    _ => {}
                }
            }
        }
        out
    }
}
