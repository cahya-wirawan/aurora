//! The Quick Select tool's engine (0.238.0): a brush-shaped stamp grows a
//! region of the active pixel layer, one stamp at a time. Read-only over
//! the store; the region it builds is the caller's to turn into a
//! selection.
//!
//! **A stamp.** The footprint is the Brush's: the texels whose centre lies
//! within `radius` of the stamp centre, plus the texel under the centre
//! (so a tiny radius still selects one). Every footprint texel is selected,
//! as a paint stroke would be. The footprint's **mean colour** (the mean of
//! each 8-bit-equivalent premultiplied channel, alpha included, rounded) is
//! the reference; the region then **grows**: a 4-connected flood fill from
//! the footprint through texels that are within `tolerance` of that mean on
//! every channel, the Magic Wand's similarity rule and units
//! ([`crate::FloodSpec`]'s tolerance). Growth never leaves a **window**
//! around the footprint: its box widened by `max(3 * radius, 24)` texels on
//! every side, clipped to the layer, so one stamp cannot flood a whole
//! photograph and a stroke has to travel to extend a selection.
//!
//! **Cheap repeats.** A stamp whose footprint is already wholly selected
//! does nothing at all and reads nothing, so dragging back and forth inside
//! flat colour costs one footprint scan per stamp. **Caps.** The selected
//! texels live in the Magic Wand's tile bitset (at most `max_tiles` tiles,
//! the caller passes [`crate::MAX_FILL_TILES`]); a stamp that would touch
//! more is refused with [`FillError::TooLarge`] having possibly merged part
//! of its growth (every merged texel is a genuine match, so the region is
//! still valid, just not complete). A window over [`MAX_QUICK_WINDOW_TEXELS`]
//! is refused before any read (not reachable at the Brush's largest radius).

use std::collections::VecDeque;

use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};

use crate::FillError;
use crate::fill::{Plane, Quantised, Region, WandRegion, region_to_wand, within_tolerance};

/// How far past the footprint, in radii, a stamp may grow.
pub const QUICK_REACH_RADII: f32 = 3.0;

/// The least reach, in texels, so a tiny brush still grows a little.
pub const QUICK_MIN_REACH: u32 = 24;

/// The most texels one stamp's window may hold (8 Mpx).
pub const MAX_QUICK_WINDOW_TEXELS: u64 = 8 * 1024 * 1024;

/// The largest radius a stamp uses (larger reads as this).
pub const MAX_QUICK_RADIUS: f32 = 1024.0;

/// The texels a Quick Select stroke has selected so far.
pub struct QuickRegion {
    region: Region,
    max_tiles: usize,
}

impl std::fmt::Debug for QuickRegion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuickRegion")
            .field("tiles", &self.region.tiles.len())
            .field("max_tiles", &self.max_tiles)
            .finish()
    }
}

impl QuickRegion {
    /// An empty region that may touch at most `max_tiles` tiles.
    #[must_use]
    pub fn new(max_tiles: usize) -> Self {
        Self {
            region: Region::new(max_tiles),
            max_tiles,
        }
    }

    /// Whether nothing is selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.region
            .tiles
            .iter()
            .all(|(_, words)| words.iter().all(|w| *w == 0))
    }

    /// Whether the layer-local texel `(x, y)` is selected.
    pub fn contains(&mut self, x: u32, y: u32) -> bool {
        self.region.contains(x, y)
    }

    /// The selected texels as a [`WandRegion`] over their tight box (`None`
    /// when there are none).
    ///
    /// # Errors
    ///
    /// [`FillError::TooLarge`] when the box is over the tile cap's area.
    pub fn to_region(&self) -> Result<Option<WandRegion>, FillError> {
        region_to_wand(&self.region, self.max_tiles)
    }
}

/// One Quick Select stamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuickStamp {
    /// The stamp's centre, layer-local texels.
    pub centre: (f32, f32),
    /// The footprint's radius in texels.
    pub radius: f32,
    /// 0..=255, 8-bit-equivalent units, against the footprint's mean.
    pub tolerance: u8,
}

/// The footprint of `stamp` clipped to `area`: the texels whose centre is
/// within the radius of the centre, plus the texel under the centre.
#[must_use]
pub fn footprint(stamp: &QuickStamp, area: (u32, u32)) -> Vec<(u32, u32)> {
    let (cx, cy) = (f64::from(stamp.centre.0), f64::from(stamp.centre.1));
    if !(cx.is_finite() && cy.is_finite() && stamp.radius.is_finite()) {
        return Vec::new();
    }
    let r = f64::from(stamp.radius.clamp(0.0, MAX_QUICK_RADIUS));
    let r2 = r * r;
    // Whole-texel bounds of the disk, clipped to the layer (the values are
    // far inside i64 and the clip makes them non-negative and below `area`).
    #[allow(clippy::cast_possible_truncation)]
    let (x_lo, x_hi, y_lo, y_hi) = (
        (cx - r - 0.5).ceil() as i64,
        (cx + r - 0.5).floor() as i64,
        (cy - r - 0.5).ceil() as i64,
        (cy + r - 0.5).floor() as i64,
    );
    let (x_lo, x_hi) = (x_lo.max(0), x_hi.min(i64::from(area.0) - 1));
    let (y_lo, y_hi) = (y_lo.max(0), y_hi.min(i64::from(area.1) - 1));
    let mut out = Vec::new();
    for y in y_lo..=y_hi {
        for x in x_lo..=x_hi {
            #[allow(clippy::cast_precision_loss)]
            let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
            if dx * dx + dy * dy <= r2
                && let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y))
            {
                out.push((x, y));
            }
        }
    }
    // The texel under the centre is always part of it.
    #[allow(clippy::cast_possible_truncation)]
    let (ux, uy) = (cx.floor() as i64, cy.floor() as i64);
    if ux >= 0
        && uy >= 0
        && ux < i64::from(area.0)
        && uy < i64::from(area.1)
        && let (Ok(ux), Ok(uy)) = (u32::try_from(ux), u32::try_from(uy))
        && !out.contains(&(ux, uy))
    {
        out.push((ux, uy));
    }
    out
}

/// The reach past the footprint, in texels, for a stamp of `radius`.
#[must_use]
pub fn reach_of(radius: f32) -> u32 {
    let r = if radius.is_finite() {
        radius.clamp(0.0, MAX_QUICK_RADIUS)
    } else {
        0.0
    };
    // Non-negative and at most 3 * 1024.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let reach = (r * QUICK_REACH_RADII).ceil() as u32;
    reach.max(QUICK_MIN_REACH)
}

/// Grows `grown` by one stamp (see the module's account). Returns whether
/// any texel was newly selected.
///
/// # Errors
///
/// [`FillError::TooLarge`] over a cap; [`FillError::Tile`] if a source tile
/// cannot be read. On an error nothing the stamp had not yet merged is
/// added.
pub fn quick_select_stamp(
    store: &mut TileStore,
    surface: SurfaceId,
    area: (u32, u32),
    stamp: &QuickStamp,
    grown: &mut QuickRegion,
) -> Result<bool, FillError> {
    let foot = footprint(stamp, area);
    if foot.is_empty() {
        return Ok(false);
    }
    if foot.iter().all(|&(x, y)| grown.contains(x, y)) {
        return Ok(false);
    }
    // The window: the footprint's box widened by the reach, clipped.
    let (mut lo, mut hi) = ((u32::MAX, u32::MAX), (0_u32, 0_u32));
    for &(x, y) in &foot {
        lo = (lo.0.min(x), lo.1.min(y));
        hi = (hi.0.max(x), hi.1.max(y));
    }
    let reach = reach_of(stamp.radius);
    let wx0 = lo.0.saturating_sub(reach);
    let wy0 = lo.1.saturating_sub(reach);
    let wx1 = hi.0.saturating_add(reach).min(area.0.saturating_sub(1));
    let wy1 = hi.1.saturating_add(reach).min(area.1.saturating_sub(1));
    let (ww, wh) = (wx1 - wx0 + 1, wy1 - wy0 + 1);
    if u64::from(ww) * u64::from(wh) > MAX_QUICK_WINDOW_TEXELS {
        return Err(FillError::TooLarge {
            max: usize::try_from(MAX_QUICK_WINDOW_TEXELS / u64::from(TILE * TILE))
                .unwrap_or(usize::MAX),
        });
    }
    let (width, height) = (ww as usize, wh as usize);
    let pixels = read_window(store, surface, (wx0, wy0), (ww, wh))?;
    // The reference: the footprint's mean colour.
    let mut sum = [0_u64; CHANNELS];
    for &(x, y) in &foot {
        let at = (y - wy0) as usize * width + (x - wx0) as usize;
        if let Some(q) = pixels.get(at) {
            for (total, channel) in sum.iter_mut().zip(q) {
                *total += u64::from(*channel);
            }
        }
    }
    let n = foot.len() as u64;
    let mut mean: Quantised = [0; CHANNELS];
    for (out, total) in mean.iter_mut().zip(sum) {
        // The mean of u8 values is at most 255.
        *out = u8::try_from((total + n / 2) / n).unwrap_or(u8::MAX);
    }
    // Flood from the footprint through the texels near the mean.
    let mut visited = vec![false; width.saturating_mul(height)];
    let mut queue: VecDeque<usize> = VecDeque::new();
    for &(x, y) in &foot {
        let at = (y - wy0) as usize * width + (x - wx0) as usize;
        if let Some(seen) = visited.get_mut(at)
            && !*seen
        {
            *seen = true;
            queue.push_back(at);
        }
    }
    while let Some(at) = queue.pop_front() {
        let (x, y) = (at % width, at / width);
        let mut visit = |nx: usize, ny: usize| {
            let n = ny * width + nx;
            if let Some(seen) = visited.get_mut(n)
                && !*seen
                && pixels
                    .get(n)
                    .is_some_and(|q| within_tolerance(mean, *q, stamp.tolerance))
            {
                *seen = true;
                queue.push_back(n);
            }
        };
        if x > 0 {
            visit(x - 1, y);
        }
        if x + 1 < width {
            visit(x + 1, y);
        }
        if y > 0 {
            visit(x, y - 1);
        }
        if y + 1 < height {
            visit(x, y + 1);
        }
    }
    // Merge the visited runs into the region.
    let mut changed = false;
    for row in 0..height {
        let y = wy0 + u32::try_from(row).unwrap_or(0);
        let mut col = 0;
        while col < width {
            if !visited.get(row * width + col).copied().unwrap_or(false) {
                col += 1;
                continue;
            }
            let start = col;
            while col < width && visited.get(row * width + col).copied().unwrap_or(false) {
                col += 1;
            }
            let x_start = wx0 + u32::try_from(start).unwrap_or(0);
            let x_end = wx0 + u32::try_from(col - 1).unwrap_or(0);
            if !changed {
                changed = (x_start..=x_end).any(|x| !grown.contains(x, y));
            }
            grown.region.set_span(y, x_start, x_end)?;
        }
    }
    Ok(changed)
}

/// The quantised texels of the layer-local box at `origin` of size `size`
/// (the box lies inside the layer); a texel of an absent tile is
/// transparent.
fn read_window(
    store: &mut TileStore,
    surface: SurfaceId,
    origin: (u32, u32),
    size: (u32, u32),
) -> Result<Vec<Quantised>, FillError> {
    let (width, height) = (size.0 as usize, size.1 as usize);
    let mut out = vec![[0_u8; CHANNELS]; width.saturating_mul(height)];
    let mut plane = Plane::new(store, surface);
    let (x_end, y_end) = (origin.0 + size.0, origin.1 + size.1);
    for ty in (origin.1 / TILE)..=((y_end - 1) / TILE) {
        for tx in (origin.0 / TILE)..=((x_end - 1) / TILE) {
            let Some(tile) = plane.tile(TileId { x: tx, y: ty })? else {
                continue;
            };
            let (tile_x0, tile_y0) = (tx * TILE, ty * TILE);
            for y in origin.1.max(tile_y0)..y_end.min(tile_y0 + TILE) {
                for x in origin.0.max(tile_x0)..x_end.min(tile_x0 + TILE) {
                    let start = ((y - tile_y0) as usize * TILE as usize + (x - tile_x0) as usize)
                        * CHANNELS;
                    let slot = (y - origin.1) as usize * width + (x - origin.0) as usize;
                    if let (Some(texel), Some(dst)) =
                        (tile.get(start..start + CHANNELS), out.get_mut(slot))
                    {
                        *dst = crate::fill::quantise(texel);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The selected region as a [`WandRegion`]; see [`QuickRegion::to_region`].
///
/// # Errors
///
/// As [`QuickRegion::to_region`].
pub fn quick_select_region(grown: &QuickRegion) -> Result<Option<WandRegion>, FillError> {
    grown.to_region()
}
