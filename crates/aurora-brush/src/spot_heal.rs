//! The Spot Healing Brush (0.237.0): paint over a blemish and, on release,
//! the painted area is filled so that it takes the colour and tone of its
//! surroundings and the texture of a similar nearby area. A simplified
//! Photoshop "Spot Healing" (content-aware / proximity style); every choice
//! below is **a choice, not compared with Photoshop**.
//!
//! **The mask.** During the stroke nothing is written: a [`HealMask`]
//! accumulates the hard-edged discs of the dabs (a texel is in when its
//! centre is strictly inside the radius; bounded by [`MAX_HEAL_TEXELS`] over
//! its bounding box and [`MAX_HEAL_SIDE`] along a side; a stroke past the
//! bound stops accumulating and is refused whole at the release).
//!
//! **The domain.** What is solved is the mask dilated by [`HEAL_FEATHER`]
//! texels (the feather needs healed values just outside the mask). The
//! *ring* is every texel with alpha > 0 within `margin = clamp(ceil(radius),
//! 8, 64)` texels (Chebyshev) of the domain and not in it. Fewer than
//! [`MIN_RING_TEXELS`] ring texels, or a part of the domain that no ring
//! texel reaches, refuses the heal ([`HealRefusal::NotEnoughSurroundings`]).
//!
//! **The source.** The best offset `(dx, dy)` within `S = clamp(4 * longest
//! side of the domain, 16, 128)` texels minimises the sum of squared
//! differences (straight RGB and alpha) between sampled ring texels (at most
//! 1,024) and the texels at the same offset; a candidate is rejected when any
//! sampled ring texel or sampled domain texel lands in the domain, outside the
//! window, or on a texel with no alpha. It is a coarse grid (step
//! `max(1, S / 24)`) refined around its best by a full local scan, the best
//! eight kept, and the first whose *whole* footprint (every domain texel and
//! every texel beside the domain) passes is taken; none gives pure diffusion.
//! Ties keep the first in scan order, so the heal is deterministic.
//!
//! **The fill.** Gradient-domain, as Photoshop's healing: with `s` the source
//! image shifted by the offset (zero when there is none) the fill is `f = s +
//! h`, where `h` is the harmonic interpolation (discrete Laplace, Dirichlet
//! `h = target - s` on the ring texels beside the domain, Neumann where a
//! neighbour has no alpha or is off the window) of the ring's difference.
//! That is the solution of `laplacian(f) = laplacian(s)` with the ring as
//! boundary: the source's texture, the target's colour and tone. The four
//! channels (straight RGB and alpha) are solved together by SOR (at most
//! [`MAX_HEAL_ITERATIONS`] sweeps, stopping when the largest change is under
//! `1e-4`). Straight colour is clamped to `0..=max(1, ring maximum)`, alpha
//! to `0..=1`, and the texel is re-premultiplied, so a premultiplied result is
//! always valid.
//!
//! **The write.** `final = old + (healed - old) * edge * selection`, with
//! `edge = max(mask, 5 x 5 box mean of the mask)` (1 on the mask, tapering to
//! 0 two texels outside). Only domain texels can change. A transparency lock
//! keeps every alpha bit for bit and sets the colour to the healed straight
//! colour at the texel's own alpha (a texel with no alpha is left alone). A
//! texel whose bits would not change is neither captured nor written, and a
//! tile the store does not hold is created only when a texel in it changes
//! (no phantom tile).

#![allow(clippy::many_single_char_names)]

use std::collections::BTreeMap;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileError, TileId, TileStore};
use half::f16;

use crate::StrokeSnapshot;
use crate::coverage::{Coverage, blend_texel, covered};
use crate::stamp::{
    DabOutcome, malformed_tile, stores_the_same_bits, texel_index, usable_snapshot,
};

/// The most texels the mask's bounding box may hold (1024 x 1024): an
/// engineering default that bounds the solve and the memory.
pub const MAX_HEAL_TEXELS: u64 = 1 << 20;
/// The longest side the mask's bounding box may have.
pub const MAX_HEAL_SIDE: u32 = 4096;
/// The fewest ring texels a heal needs.
pub const MIN_RING_TEXELS: usize = 16;
/// How far the healed domain (and the soft edge) reaches outside the mask.
pub const HEAL_FEATHER: i64 = 2;
/// The most SOR sweeps a solve makes.
pub const MAX_HEAL_ITERATIONS: usize = 400;

const TOLERANCE: f32 = 1.0e-4;
const NONE: u32 = u32::MAX;
const MAX_SAMPLES: usize = 1024;
const KEEP_CANDIDATES: usize = 8;
const MAX_RADIUS: f32 = 1024.0;
const MAX_CENTER: f32 = 2_147_483_648.0;

/// Why a heal was refused (nothing is written).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealRefusal {
    /// The painted area is over [`MAX_HEAL_TEXELS`] or [`MAX_HEAL_SIDE`].
    TooLarge,
    /// Too few surrounding texels, or part of the area has none within reach.
    NotEnoughSurroundings,
}

/// The area a Spot Healing stroke has painted so far: hard-edged discs in
/// layer-local texels, in a bounding box that grows with the stroke.
#[derive(Debug, Clone, Default)]
pub struct HealMask {
    x0: i64,
    y0: i64,
    w: u32,
    h: u32,
    bits: Vec<u8>,
    count: u64,
    overflow: bool,
}

impl HealMask {
    /// An empty mask.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether no texel is painted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// How many texels are painted.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Whether the stroke outgrew the bound and stopped accumulating.
    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.overflow
    }

    /// Whether layer-local texel `(x, y)` is painted.
    #[must_use]
    pub fn contains(&self, x: i64, y: i64) -> bool {
        let (rx, ry) = (x - self.x0, y - self.y0);
        if rx < 0 || ry < 0 || rx >= i64::from(self.w) || ry >= i64::from(self.h) {
            return false;
        }
        #[allow(clippy::cast_sign_loss)]
        let index = (ry as usize) * (self.w as usize) + rx as usize;
        self.bits.get(index).is_some_and(|&b| b != 0)
    }

    /// The tight bounds `(x0, y0, x1, y1)` (inclusive) of the painted
    /// texels, `None` when empty.
    #[must_use]
    pub fn tight_bounds(&self) -> Option<(i64, i64, i64, i64)> {
        if self.count == 0 {
            return None;
        }
        let w = self.w as usize;
        let mut out: Option<(i64, i64, i64, i64)> = None;
        for (row, line) in self.bits.chunks(w.max(1)).enumerate() {
            for (col, &b) in line.iter().enumerate() {
                if b == 0 {
                    continue;
                }
                #[allow(clippy::cast_possible_wrap)]
                let (x, y) = (self.x0 + col as i64, self.y0 + row as i64);
                out = Some(match out {
                    None => (x, y, x, y),
                    Some((a, b2, c, d)) => (a.min(x), b2.min(y), c.max(x), d.max(y)),
                });
            }
        }
        out
    }

    /// Adds the hard disc of a dab at `center` (layer-local texels) with
    /// `radius`: every texel whose centre is strictly inside. Does nothing
    /// for a non-finite or non-positive input, or after an overflow; a dab
    /// that would grow the bounding box past [`MAX_HEAL_TEXELS`] or
    /// [`MAX_HEAL_SIDE`] sets the overflow flag instead and adds nothing.
    /// Texels left of or above the layer's origin do not exist and are
    /// skipped.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        clippy::cast_possible_wrap
    )]
    pub fn add_dab(&mut self, center: (f32, f32), radius: f32) {
        if self.overflow
            || !(center.0.is_finite() && center.1.is_finite() && radius.is_finite())
            || radius <= 0.0
            || center.0.abs() > MAX_CENTER
            || center.1.abs() > MAX_CENTER
        {
            return;
        }
        let radius = radius.min(MAX_RADIUS);
        let lo_x = (center.0 - radius).floor().max(0.0) as i64;
        let lo_y = (center.1 - radius).floor().max(0.0) as i64;
        let hi_x = (center.0 + radius).ceil() as i64;
        let hi_y = (center.1 + radius).ceil() as i64;
        if hi_x < lo_x || hi_y < lo_y {
            return;
        }
        // Where the new bounding box would be.
        let (nx0, ny0, nx1, ny1) = if self.bits.is_empty() {
            (lo_x, lo_y, hi_x, hi_y)
        } else {
            (
                self.x0.min(lo_x),
                self.y0.min(lo_y),
                (self.x0 + i64::from(self.w) - 1).max(hi_x),
                (self.y0 + i64::from(self.h) - 1).max(hi_y),
            )
        };
        let (nw, nh) = (nx1 - nx0 + 1, ny1 - ny0 + 1);
        if nw > i64::from(MAX_HEAL_SIDE)
            || nh > i64::from(MAX_HEAL_SIDE)
            || (nw as u64) * (nh as u64) > MAX_HEAL_TEXELS
        {
            self.overflow = true;
            return;
        }
        if self.bits.is_empty()
            || (nx0, ny0, nw, nh) != (self.x0, self.y0, i64::from(self.w), i64::from(self.h))
        {
            let mut bits = vec![0_u8; (nw as usize) * (nh as usize)];
            if !self.bits.is_empty() {
                let old_w = self.w as usize;
                let dx = (self.x0 - nx0) as usize;
                let dy = (self.y0 - ny0) as usize;
                for (row, line) in self.bits.chunks(old_w.max(1)).enumerate() {
                    let start = (row + dy) * (nw as usize) + dx;
                    if let Some(dst) = bits.get_mut(start..start + line.len()) {
                        dst.copy_from_slice(line);
                    }
                }
            }
            self.bits = bits;
            self.x0 = nx0;
            self.y0 = ny0;
            self.w = nw as u32;
            self.h = nh as u32;
        }
        let r2 = radius * radius;
        let width = self.w as usize;
        for y in lo_y..=hi_y {
            let dy = y as f32 + 0.5 - center.1;
            for x in lo_x..=hi_x {
                let dx = x as f32 + 0.5 - center.0;
                if dx * dx + dy * dy >= r2 {
                    continue;
                }
                let index = ((y - self.y0) as usize) * width + (x - self.x0) as usize;
                if let Some(bit) = self.bits.get_mut(index)
                    && *bit == 0
                {
                    *bit = 1;
                    self.count += 1;
                }
            }
        }
    }
}

/// A rectangle of the layer read as premultiplied `f32` (a transparent
/// texel for anything the layer does not hold).
pub(crate) struct Window {
    pub(crate) w: usize,
    pub(crate) h: usize,
    /// `w * h` texels of four channels.
    pub(crate) texels: Vec<f32>,
}

impl Window {
    fn alpha(&self, i: usize) -> f32 {
        self.texels.get(i * CHANNELS + 3).copied().unwrap_or(0.0)
    }

    fn valid(&self, i: usize) -> bool {
        self.alpha(i) > 0.0
    }

    /// Straight RGB and alpha of texel `i` (zeros without alpha).
    fn straight(&self, i: usize) -> [f32; 4] {
        let Some(t) = self.texels.get(i * CHANNELS..i * CHANNELS + CHANNELS) else {
            return [0.0; 4];
        };
        let a = t.get(3).copied().unwrap_or(0.0);
        if a <= 0.0 {
            return [0.0; 4];
        }
        [
            t.first().copied().unwrap_or(0.0) / a,
            t.get(1).copied().unwrap_or(0.0) / a,
            t.get(2).copied().unwrap_or(0.0) / a,
            a,
        ]
    }

    /// Index of texel `(x, y)` when inside the window.
    fn index(&self, x: i64, y: i64) -> Option<usize> {
        if x < 0 || y < 0 {
            return None;
        }
        #[allow(clippy::cast_sign_loss)]
        let (ux, uy) = (x as usize, y as usize);
        (ux < self.w && uy < self.h).then(|| uy * self.w + ux)
    }
}

/// Reads the layer rectangle `(x0, y0, w, h)` (layer-local, non-negative)
/// as premultiplied `f32`; `Err` is the tile that could not be read.
fn gather_rect(
    store: &mut TileStore,
    surface: SurfaceId,
    x0: i64,
    y0: i64,
    w: usize,
    h: usize,
) -> Result<Window, (TileId, TileError)> {
    let mut texels = vec![0.0_f32; w * h * CHANNELS];
    let tile = i64::from(TILE);
    #[allow(clippy::cast_possible_wrap)]
    let (x1, y1) = (x0 + w as i64 - 1, y0 + h as i64 - 1);
    let max_tile = i64::from(u32::MAX / TILE);
    let (tx0, tx1) = ((x0 / tile).min(max_tile), (x1 / tile).min(max_tile));
    let (ty0, ty1) = ((y0 / tile).min(max_tile), (y1 / tile).min(max_tile));
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let id = TileId {
                x: tx as u32,
                y: ty as u32,
            };
            if !store.contains_tile(surface, id) {
                continue;
            }
            let source = match store.get(surface, id) {
                Ok(tile) => tile.texels(),
                Err(err) => return Err((id, err)),
            };
            let (ox0, oy0) = (x0.max(tx * tile), y0.max(ty * tile));
            let (ox1, oy1) = (x1.min(tx * tile + tile - 1), y1.min(ty * tile + tile - 1));
            for y in oy0..=oy1 {
                for x in ox0..=ox1 {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let from = texel_index((x - tx * tile) as u32, (y - ty * tile) as u32);
                    #[allow(clippy::cast_sign_loss)]
                    let to = (((y - y0) as usize) * w + (x - x0) as usize) * CHANNELS;
                    if let (Some(src), Some(dst)) = (
                        source.get(from..from + CHANNELS),
                        texels.get_mut(to..to + CHANNELS),
                    ) {
                        for (d, s) in dst.iter_mut().zip(src) {
                            let v = s.to_f32();
                            *d = if v.is_finite() { v } else { 0.0 };
                        }
                    }
                }
            }
        }
    }
    Ok(Window { w, h, texels })
}

/// `src` dilated by Chebyshev radius `r` inside a `w x h` grid.
fn dilate(src: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let mut rows = vec![false; w * h];
    for (line, out) in src.chunks(w.max(1)).zip(rows.chunks_mut(w.max(1))) {
        let mut prefix = Vec::with_capacity(w + 1);
        prefix.push(0_u32);
        let mut sum = 0_u32;
        for &b in line {
            sum += u32::from(b);
            prefix.push(sum);
        }
        for (x, slot) in out.iter_mut().enumerate() {
            let lo = x.saturating_sub(r);
            let hi = (x + r + 1).min(w);
            *slot = prefix.get(hi).copied().unwrap_or(0) > prefix.get(lo).copied().unwrap_or(0);
        }
    }
    let mut cols = vec![0_u32; (h + 1) * w];
    for y in 0..h {
        for x in 0..w {
            let above = cols.get(y * w + x).copied().unwrap_or(0);
            let here = u32::from(rows.get(y * w + x).copied().unwrap_or(false));
            if let Some(slot) = cols.get_mut((y + 1) * w + x) {
                *slot = above + here;
            }
        }
    }
    let mut out = vec![false; w * h];
    for y in 0..h {
        let (lo, hi) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let a = cols.get(hi * w + x).copied().unwrap_or(0);
            let b = cols.get(lo * w + x).copied().unwrap_or(0);
            if let Some(slot) = out.get_mut(y * w + x) {
                *slot = a > b;
            }
        }
    }
    out
}

/// One texel to write: its index in the tile's samples and its new value.
type TileWrite = (usize, [f16; CHANNELS]);

const DIRS: [(i64, i64); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

/// Offsets within the search window, for a domain whose longest side is
/// `side`.
fn search_radius(side: usize) -> i64 {
    #[allow(clippy::cast_possible_wrap)]
    let s = (side as i64).saturating_mul(4);
    s.clamp(16, 128)
}

/// The ring margin for a brush `radius`.
fn margin_for(radius: f32) -> i64 {
    if !radius.is_finite() {
        return 8;
    }
    #[allow(clippy::cast_possible_truncation)]
    let r = radius.ceil().clamp(8.0, 64.0) as i64;
    r
}

struct Search<'a> {
    win: &'a Window,
    domain: &'a [bool],
    ring_samples: Vec<(usize, [f32; 4])>,
    domain_samples: Vec<usize>,
}

impl Search<'_> {
    /// The window index of the source texel for `i + offset`, when usable.
    fn source(&self, i: usize, offset: (i64, i64)) -> Option<usize> {
        let (w, _) = (self.win.w, self.win.h);
        #[allow(clippy::cast_possible_wrap)]
        let (x, y) = ((i % w) as i64 + offset.0, (i / w) as i64 + offset.1);
        let j = self.win.index(x, y)?;
        (!self.domain.get(j).copied().unwrap_or(true) && self.win.valid(j)).then_some(j)
    }

    /// The SSD of the sampled ring against the ring shifted by `offset`, or
    /// `None` when the candidate is unusable or already worse than `limit`.
    fn score(&self, offset: (i64, i64), limit: f32) -> Option<f32> {
        let mut sum = 0.0_f32;
        for &(i, target) in &self.ring_samples {
            let j = self.source(i, offset)?;
            let s = self.win.straight(j);
            for (a, b) in target.iter().zip(&s) {
                sum += (a - b) * (a - b);
            }
            if sum > limit {
                return None;
            }
        }
        for &i in &self.domain_samples {
            self.source(i, offset)?;
        }
        Some(sum)
    }
}

/// Keeps `list` the best `KEEP_CANDIDATES` by score, earlier entries first
/// on equal scores.
fn offer(list: &mut Vec<(f32, (i64, i64))>, score: f32, offset: (i64, i64)) {
    if list.iter().any(|&(_, o)| o == offset) {
        return;
    }
    let at = list
        .iter()
        .position(|&(s, _)| s > score)
        .unwrap_or(list.len());
    list.insert(at, (score, offset));
    list.truncate(KEEP_CANDIDATES);
}

fn every_nth<T: Copy>(items: &[T], cap: usize) -> Vec<T> {
    let stride = items.len().div_ceil(cap.max(1)).max(1);
    items.iter().step_by(stride).copied().collect()
}

/// The healed straight RGBA of every domain texel (window index and value),
/// in row-major order.
///
/// `domain` marks the texels to solve (`w * h` flags); `margin` is the ring
/// width.
#[allow(
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]
pub(crate) fn heal_window(
    win: &Window,
    domain: &[bool],
    margin: usize,
) -> Result<Vec<(usize, [f32; 4])>, HealRefusal> {
    let (w, h) = (win.w, win.h);
    if domain.len() != w * h {
        return Err(HealRefusal::NotEnoughSurroundings);
    }
    let mut slot = vec![NONE; w * h];
    let mut unknown: Vec<usize> = Vec::new();
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0, 0);
    for (i, &inside) in domain.iter().enumerate() {
        if !inside {
            continue;
        }
        if let Some(s) = slot.get_mut(i) {
            *s = unknown.len() as u32;
        }
        unknown.push(i);
        let (x, y) = (i % w, i / w);
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    if unknown.is_empty() {
        return Ok(Vec::new());
    }
    let side = (max_x - min_x + 1).max(max_y - min_y + 1);

    // The ring.
    let near = dilate(domain, w, h, margin);
    let ring: Vec<usize> = (0..w * h)
        .filter(|&i| {
            near.get(i).copied().unwrap_or(false)
                && !domain.get(i).copied().unwrap_or(true)
                && win.valid(i)
        })
        .collect();
    if ring.len() < MIN_RING_TEXELS {
        return Err(HealRefusal::NotEnoughSurroundings);
    }

    // Neighbours: other unknowns, ring texels (Dirichlet) or nothing (Neumann).
    let mut unknown_nb = vec![[NONE; 4]; unknown.len()];
    let mut boundary_nb = vec![[NONE; 4]; unknown.len()];
    for (k, &i) in unknown.iter().enumerate() {
        #[allow(clippy::cast_possible_wrap)]
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        for (d, (ox, oy)) in DIRS.iter().enumerate() {
            let Some(j) = win.index(x + ox, y + oy) else {
                continue;
            };
            if domain.get(j).copied().unwrap_or(false) {
                if let Some(s) = unknown_nb.get_mut(k).and_then(|a| a.get_mut(d)) {
                    *s = slot.get(j).copied().unwrap_or(NONE);
                }
            } else if win.valid(j)
                && let Some(s) = boundary_nb.get_mut(k).and_then(|a| a.get_mut(d))
            {
                *s = j as u32;
            }
        }
    }
    // Every unknown must be connected to a boundary texel.
    let mut reached = vec![false; unknown.len()];
    let mut queue: Vec<usize> = Vec::new();
    for (k, nb) in boundary_nb.iter().enumerate() {
        if nb.iter().any(|&j| j != NONE) {
            if let Some(r) = reached.get_mut(k) {
                *r = true;
            }
            queue.push(k);
        }
    }
    while let Some(k) = queue.pop() {
        for &j in unknown_nb.get(k).map_or(&[NONE; 4], |a| a) {
            if j == NONE {
                continue;
            }
            let j = j as usize;
            if let Some(r) = reached.get_mut(j)
                && !*r
            {
                *r = true;
                queue.push(j);
            }
        }
    }
    if reached.iter().any(|&r| !r) {
        return Err(HealRefusal::NotEnoughSurroundings);
    }

    // The source offset.
    let search = Search {
        win,
        domain,
        ring_samples: every_nth(&ring, MAX_SAMPLES)
            .into_iter()
            .map(|i| (i, win.straight(i)))
            .collect(),
        domain_samples: every_nth(&unknown, MAX_SAMPLES),
    };
    let offset = find_source(&search, &unknown, &boundary_nb, side);

    // The harmonic part.
    let shifted = |i: usize| -> [f32; 4] {
        offset.map_or([0.0; 4], |o| {
            search.source(i, o).map_or([0.0; 4], |j| win.straight(j))
        })
    };
    let mut bsum = vec![[0.0_f32; 4]; unknown.len()];
    let mut inv_n = vec![0.0_f32; unknown.len()];
    let (mut total, mut count) = ([0.0_f32; 4], 0.0_f32);
    for (k, (ub, bb)) in unknown_nb.iter().zip(&boundary_nb).enumerate() {
        let mut n = 0.0_f32;
        let mut sum = [0.0_f32; 4];
        for &j in ub {
            if j != NONE {
                n += 1.0;
            }
        }
        for &q in bb {
            if q == NONE {
                continue;
            }
            n += 1.0;
            let q = q as usize;
            let (t, s) = (win.straight(q), shifted(q));
            for c in 0..4 {
                let v = t.get(c).copied().unwrap_or(0.0) - s.get(c).copied().unwrap_or(0.0);
                if let Some(slot) = sum.get_mut(c) {
                    *slot += v;
                }
                if let Some(slot) = total.get_mut(c) {
                    *slot += v;
                }
            }
            count += 1.0;
        }
        if let Some(s) = bsum.get_mut(k) {
            *s = sum;
        }
        if let Some(s) = inv_n.get_mut(k) {
            *s = if n > 0.0 { 1.0 / n } else { 0.0 };
        }
    }
    let mean = total.map(|t| if count > 0.0 { t / count } else { 0.0 });
    let mut field = vec![mean; unknown.len()];
    let omega = (2.0 / (1.0 + (std::f32::consts::PI / (side as f32 + 1.0)).sin())).clamp(1.0, 1.98);
    for _ in 0..MAX_HEAL_ITERATIONS {
        let mut largest = 0.0_f32;
        for k in 0..unknown.len() {
            let mut acc = bsum.get(k).copied().unwrap_or([0.0; 4]);
            for &j in unknown_nb.get(k).map_or(&[NONE; 4], |a| a) {
                if j == NONE {
                    continue;
                }
                let other = field.get(j as usize).copied().unwrap_or([0.0; 4]);
                for (a, o) in acc.iter_mut().zip(&other) {
                    *a += o;
                }
            }
            let scale = inv_n.get(k).copied().unwrap_or(0.0);
            if let Some(cur) = field.get_mut(k) {
                for (c, a) in cur.iter_mut().zip(&acc) {
                    let target = a * scale;
                    let step = omega * (target - *c);
                    *c += step;
                    largest = largest.max(step.abs());
                }
            }
        }
        if largest < TOLERANCE {
            break;
        }
    }

    // The result.
    let mut hi = 1.0_f32;
    for &i in &ring {
        let s = win.straight(i);
        for v in s.iter().take(3) {
            hi = hi.max(*v);
        }
    }
    let mut out = Vec::with_capacity(unknown.len());
    for (k, &i) in unknown.iter().enumerate() {
        let s = shifted(i);
        let f = field.get(k).copied().unwrap_or([0.0; 4]);
        let mut v = [0.0_f32; 4];
        for c in 0..4 {
            let x = s.get(c).copied().unwrap_or(0.0) + f.get(c).copied().unwrap_or(0.0);
            let x = if x.is_finite() { x } else { 0.0 };
            if let Some(slot) = v.get_mut(c) {
                *slot = if c == 3 {
                    x.clamp(0.0, 1.0)
                } else {
                    x.clamp(0.0, hi)
                };
            }
        }
        out.push((i, v));
    }
    Ok(out)
}

/// The best source offset, if any (see the module docs).
fn find_source(
    search: &Search<'_>,
    unknown: &[usize],
    boundary_nb: &[[u32; 4]],
    side: usize,
) -> Option<(i64, i64)> {
    #[allow(clippy::cast_possible_wrap)]
    let s = search_radius(side);
    let step = (s / 24).max(1);
    let mut best: Vec<(f32, (i64, i64))> = Vec::new();
    let limit_of = |best: &Vec<(f32, (i64, i64))>| {
        if best.len() >= KEEP_CANDIDATES {
            best.last().map_or(f32::INFINITY, |&(sc, _)| sc)
        } else {
            f32::INFINITY
        }
    };
    let n = s / step;
    for ky in -n..=n {
        for kx in -n..=n {
            let offset = (kx * step, ky * step);
            if let Some(score) = search.score(offset, limit_of(&best)) {
                offer(&mut best, score, offset);
            }
        }
    }
    if step > 1
        && let Some(&(_, (bx, by))) = best.first()
    {
        for dy in (1 - step)..step {
            for dx in (1 - step)..step {
                let offset = (bx + dx, by + dy);
                if let Some(score) = search.score(offset, limit_of(&best)) {
                    offer(&mut best, score, offset);
                }
            }
        }
    }
    best.into_iter().map(|(_, o)| o).find(|&o| {
        unknown.iter().zip(boundary_nb).all(|(&i, nb)| {
            search.source(i, o).is_some()
                && nb
                    .iter()
                    .all(|&q| q == NONE || search.source(q as usize, o).is_some())
        })
    })
}

/// Heals the area `mask` holds in the layer `surface` of `store` (see the
/// module docs). `radius` is the brush radius (it sets the ring width),
/// `preserve_alpha` the transparency lock, `snapshot` the stroke's undo
/// snapshot (it captures every tile before its first write), `coverage` the
/// active selection. An unreadable tile is reported in the outcome's
/// [`DabOutcome::failed`] and nothing is written. An empty mask does nothing.
///
/// # Errors
///
/// [`HealRefusal`] when the area is too large or has too little around it;
/// nothing is written.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
pub fn spot_heal_covered(
    store: &mut TileStore,
    surface: SurfaceId,
    mask: &HealMask,
    radius: f32,
    preserve_alpha: bool,
    snapshot: Option<&mut StrokeSnapshot>,
    coverage: Option<&dyn Coverage>,
) -> Result<DabOutcome, HealRefusal> {
    if mask.overflowed() {
        return Err(HealRefusal::TooLarge);
    }
    let mut outcome = DabOutcome::default();
    let Some((tx0, ty0, tx1, ty1)) = mask.tight_bounds() else {
        return Ok(outcome);
    };
    let (bw, bh) = (tx1 - tx0 + 1, ty1 - ty0 + 1);
    if bw > i64::from(MAX_HEAL_SIDE)
        || bh > i64::from(MAX_HEAL_SIDE)
        || (bw as u64) * (bh as u64) > MAX_HEAL_TEXELS
    {
        return Err(HealRefusal::TooLarge);
    }
    let f = HEAL_FEATHER;
    let (dx0, dy0, dx1, dy1) = ((tx0 - f).max(0), (ty0 - f).max(0), tx1 + f, ty1 + f);
    let side = (dx1 - dx0 + 1).max(dy1 - dy0 + 1) as usize;
    let margin = margin_for(radius);
    let reach = margin + search_radius(side) + 1;
    let (wx0, wy0) = ((dx0 - reach).max(0), (dy0 - reach).max(0));
    let (wx1, wy1) = (dx1 + reach, dy1 + reach);
    let (ww, wh) = ((wx1 - wx0 + 1) as usize, (wy1 - wy0 + 1) as usize);
    let win = match gather_rect(store, surface, wx0, wy0, ww, wh) {
        Ok(win) => win,
        Err((id, err)) => {
            outcome.fail(id, err);
            return Ok(outcome);
        }
    };

    // The mask in window coordinates, the domain and the soft edge.
    let mut inmask = vec![false; ww * wh];
    for y in ty0..=ty1 {
        for x in tx0..=tx1 {
            if mask.contains(x, y)
                && let Some(s) = inmask.get_mut(((y - wy0) as usize) * ww + (x - wx0) as usize)
            {
                *s = true;
            }
        }
    }
    let domain = dilate(&inmask, ww, wh, f as usize);
    let healed = heal_window(&win, &domain, margin as usize)?;

    // 5 x 5 box mean of the mask through an integral image.
    let mut integral = vec![0_u32; (ww + 1) * (wh + 1)];
    for y in 0..wh {
        let mut run = 0_u32;
        for x in 0..ww {
            run += u32::from(inmask.get(y * ww + x).copied().unwrap_or(false));
            let above = integral.get(y * (ww + 1) + x + 1).copied().unwrap_or(0);
            if let Some(s) = integral.get_mut((y + 1) * (ww + 1) + x + 1) {
                *s = above + run;
            }
        }
    }
    let box_count = |x: usize, y: usize| -> u32 {
        let r = f as usize;
        let (x0, y0) = (x.saturating_sub(r), y.saturating_sub(r));
        let (x1, y1) = ((x + r + 1).min(ww), (y + r + 1).min(wh));
        let at = |xx: usize, yy: usize| integral.get(yy * (ww + 1) + xx).copied().unwrap_or(0);
        (at(x1, y1) + at(x0, y0)).saturating_sub(at(x0, y1) + at(x1, y0))
    };
    let area = ((2 * f + 1) * (2 * f + 1)) as f32;

    // What changes, tile by tile.
    let mut writes: BTreeMap<(u32, u32), Vec<TileWrite>> = BTreeMap::new();
    for (i, healed_texel) in healed {
        let (lx, ly) = (i % ww, i / ww);
        let (x, y) = (wx0 + lx as i64, wy0 + ly as i64);
        let mut edge = if inmask.get(i).copied().unwrap_or(false) {
            1.0
        } else {
            box_count(lx, ly) as f32 / area
        };
        if let Some(cov) = coverage {
            edge *= covered(cov, x, y);
        }
        if edge <= 0.0 {
            continue;
        }
        let t = win.texels.get(i * CHANNELS..i * CHANNELS + CHANNELS);
        let old: [f16; CHANNELS] = match t {
            Some(t) => [
                f16::from_f32(t.first().copied().unwrap_or(0.0)),
                f16::from_f32(t.get(1).copied().unwrap_or(0.0)),
                f16::from_f32(t.get(2).copied().unwrap_or(0.0)),
                f16::from_f32(t.get(3).copied().unwrap_or(0.0)),
            ],
            None => continue,
        };
        let [r, g, b, a] = healed_texel;
        let fresh = if preserve_alpha {
            let keep = old[3].to_f32();
            if keep <= 0.0 {
                continue;
            }
            [
                f16::from_f32(r * keep),
                f16::from_f32(g * keep),
                f16::from_f32(b * keep),
                old[3],
            ]
        } else {
            [
                f16::from_f32(r * a),
                f16::from_f32(g * a),
                f16::from_f32(b * a),
                f16::from_f32(a),
            ]
        };
        let fresh = if edge < 1.0 {
            blend_texel(&old, fresh, edge)
        } else {
            fresh
        };
        if stores_the_same_bits(&old, fresh) {
            continue;
        }
        let (tile_x, tile_y) = ((x as u32) / TILE, (y as u32) / TILE);
        let local = texel_index((x as u32) % TILE, (y as u32) % TILE);
        writes
            .entry((tile_x, tile_y))
            .or_default()
            .push((local, fresh));
    }

    let mut snapshot = usable_snapshot(snapshot, surface);
    for ((tile_x, tile_y), list) in writes {
        let id = TileId {
            x: tile_x,
            y: tile_y,
        };
        let tile = match store.get_mut(surface, id) {
            Ok(tile) => tile,
            Err(err) => {
                outcome.fail(id, err);
                continue;
            }
        };
        let texels = tile.texels_mut();
        if let Some(stroke) = snapshot.as_mut()
            && !stroke.record_content(id, texels)
        {
            outcome.fail(id, malformed_tile(surface, id, texels.len()));
            continue;
        }
        let (mut lx0, mut ly0, mut lx1, mut ly1) = (u32::MAX, u32::MAX, 0_u32, 0_u32);
        for (local, fresh) in list {
            for (c, &v) in fresh.iter().enumerate() {
                if let Some(s) = texels.get_mut(local + c) {
                    *s = v;
                }
            }
            let texel = (local / CHANNELS) as u32;
            let (x, y) = (texel % TILE, texel / TILE);
            lx0 = lx0.min(x);
            ly0 = ly0.min(y);
            lx1 = lx1.max(x);
            ly1 = ly1.max(y);
        }
        outcome.painted.push(id);
        tile.mark_dirty(Rect {
            x: i64::from(lx0),
            y: i64::from(ly0),
            width: lx1 - lx0 + 1,
            height: ly1 - ly0 + 1,
        });
    }
    Ok(outcome)
}
