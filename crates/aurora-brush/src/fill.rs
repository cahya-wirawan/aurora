//! The Paint Bucket and the Gradient (0.209.0): two whole-region fills of
//! one pixel layer, written through the same tile store, with the same
//! per-tile undo capture ([`StrokeSnapshot::record_content`]) a brush
//! stroke uses, so one fill is one [`crate::PixelHistory`] step.
//!
//! **Coordinates.** Everything here is *layer-local*: pixel `(0, 0)` is the
//! top-left of the layer's own surface (what `aurora-app` gets by
//! subtracting the layer's bounds origin), and a fill is confined to
//! `[0, width) x [0, height)` of the `area` it is given (the layer's
//! bounds, which the caller may clip further). Nothing is read or written
//! outside it.
//!
//! **Colours.** Straight, encoded sRGB `[r, g, b]` in `[0, 1]`, the space
//! the foreground/background swatches and the Brush work in. A tile stores
//! premultiplied RGBA in `f16`, so an opaque fill writes `[r, g, b, 1]`.
//! The result is stored as `f16`, so no dithering is needed.
//!
//! **Bounded work (invariant 7.3.1).** Nothing here allocates a whole-layer
//! image. A flood fill keeps a bit per examined-and-matched pixel in
//! 8 KiB per-tile bitsets, reads source tiles one at a time through a
//! small cache, and refuses with [`FillError::TooLarge`] — before writing
//! a single texel — as soon as the region would touch more than
//! [`MAX_FILL_TILES`] tiles (1024 tiles, 64 Mpx, at most 512 MiB of undo
//! snapshot). A gradient refuses the same way when its area spans more
//! tiles. A huge empty layer therefore costs at most `MAX_FILL_TILES`
//! tiles of scanning and no memory beyond the bitsets.
//!
//! **Tolerance** is in 8-bit-equivalent units, as Photoshop's: each
//! premultiplied RGBA channel is quantised to `round(v * 255)` in
//! `0..=255` and a pixel matches the clicked pixel when no channel
//! differs by more than the tolerance. Tolerance `0` is exact 8-bit
//! equality, `255` matches everything. Premultiplied, so a fully
//! transparent pixel is the same colour whatever its stale RGB.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SAMPLES, SurfaceId, TILE, TileError, TileId, TileStore};
use half::f16;

use crate::StrokeSnapshot;
use crate::coverage::{Coverage, TileCover, apply_coverage, covered, tile_cover};

/// The most tiles one fill may touch: 1024 tiles (64 Mpx). A larger flood
/// region or gradient area is refused, untouched, with
/// [`FillError::TooLarge`].
pub const MAX_FILL_TILES: usize = 1024;

/// The most **existing** tiles a non-contiguous flood fill may read while
/// looking for similar pixels (4096 tiles, 268 Mpx of scanning): a layer
/// holding more is refused with [`FillError::TooLarge`] before any write.
/// An engineering default like [`MAX_FILL_TILES`], and higher because a
/// scanned tile that does not match costs time, not undo memory.
pub const MAX_EXAMINED_TILES: usize = 4096;

/// Tiles of source pixels a flood fill keeps decoded at once.
const READ_CACHE_TILES: usize = 32;

const TILE_SIDE: usize = TILE as usize;
/// `u64` words in one tile's bitset.
const WORDS: usize = TILE_SIDE * TILE_SIDE / 64;

/// How a fill treats the layer's alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillMode {
    /// Replaces covered pixels (Paint Bucket) or blends over them at the
    /// gradient's opacity.
    Normal,
    /// Transparency lock: recolours existing pixels and never changes a
    /// texel's alpha; fully transparent pixels are untouched.
    PreserveAlpha,
}

/// Why a fill did nothing.
#[derive(Debug, thiserror::Error)]
pub enum FillError {
    /// The fill would touch more than `max` tiles. Nothing was written.
    #[error("the fill would touch more than {max} tiles")]
    TooLarge {
        /// The cap that was exceeded.
        max: usize,
    },
    /// A source tile could not be read. Nothing was written.
    #[error("a tile could not be read: {0}")]
    Tile(#[from] TileError),
}

/// What a fill wrote.
#[derive(Debug, Default)]
#[must_use]
pub struct FillOutcome {
    painted: Vec<TileId>,
    failed: Vec<(TileId, TileError)>,
}

impl FillOutcome {
    /// Tiles at least one texel of which changed — exactly the tiles the
    /// snapshot captured.
    #[must_use]
    pub fn painted(&self) -> &[TileId] {
        &self.painted
    }

    /// Tiles that could not be paged in for writing; each is untouched.
    #[must_use]
    pub fn failed(&self) -> &[(TileId, TileError)] {
        &self.failed
    }
}

/// One premultiplied texel in tile channel order.
type Texel = [f16; CHANNELS];

/// A texel's channels in 8-bit-equivalent units.
type Quantised = [u8; CHANNELS];

#[allow(clippy::cast_sign_loss)]
fn quantise_channel(value: f16) -> u8 {
    // A NaN saturates to 0; an out-of-gamut value clamps.
    (value.to_f32() * 255.0).round().clamp(0.0, 255.0) as u8
}

fn quantise(texel: &[f16]) -> Quantised {
    let at = |i: usize| texel.get(i).copied().map_or(0, quantise_channel);
    [at(0), at(1), at(2), at(3)]
}

/// Whether `candidate` is within `tolerance` of `seed` on every channel.
fn within_tolerance(seed: Quantised, candidate: Quantised, tolerance: u8) -> bool {
    seed.iter()
        .zip(candidate.iter())
        .all(|(a, b)| a.abs_diff(*b) <= tolerance)
}

/// Layer-local pixel to (tile, tile-local x, tile-local y).
fn split(x: u32, y: u32) -> (TileId, usize, usize) {
    (
        TileId {
            x: x / TILE,
            y: y / TILE,
        },
        (x % TILE) as usize,
        (y % TILE) as usize,
    )
}

/// Reads a layer's source pixels one decoded tile at a time. A tile the
/// store does not hold reads as fully transparent without being created.
struct Plane<'a> {
    store: &'a mut TileStore,
    surface: SurfaceId,
    cache: HashMap<TileId, Option<Rc<Vec<f16>>>>,
    order: VecDeque<TileId>,
    last: Option<(TileId, Option<Rc<Vec<f16>>>)>,
}

impl<'a> Plane<'a> {
    fn new(store: &'a mut TileStore, surface: SurfaceId) -> Self {
        Self {
            store,
            surface,
            cache: HashMap::new(),
            order: VecDeque::new(),
            last: None,
        }
    }

    fn tile(&mut self, id: TileId) -> Result<Option<Rc<Vec<f16>>>, TileError> {
        if let Some((last_id, tile)) = &self.last
            && *last_id == id
        {
            return Ok(tile.clone());
        }
        let tile = if let Some(hit) = self.cache.get(&id) {
            hit.clone()
        } else {
            let loaded = if self.store.contains_tile(self.surface, id) {
                Some(Rc::new(self.store.get(self.surface, id)?.texels().to_vec()))
            } else {
                None
            };
            if self.cache.len() >= READ_CACHE_TILES
                && let Some(oldest) = self.order.pop_front()
            {
                self.cache.remove(&oldest);
            }
            self.order.push_back(id);
            self.cache.insert(id, loaded.clone());
            loaded
        };
        self.last = Some((id, tile.clone()));
        Ok(tile)
    }

    fn quantised(&mut self, x: u32, y: u32) -> Result<Quantised, TileError> {
        let (id, lx, ly) = split(x, y);
        let Some(tile) = self.tile(id)? else {
            return Ok([0; CHANNELS]);
        };
        let start = (ly * TILE_SIDE + lx) * CHANNELS;
        Ok(tile
            .get(start..start + CHANNELS)
            .map_or([0; CHANNELS], quantise))
    }
}

/// The set of matched pixels, as one bitset per touched tile.
struct Region {
    tiles: Vec<(TileId, Box<[u64]>)>,
    index: HashMap<TileId, usize>,
    last: Option<(TileId, usize)>,
    max_tiles: usize,
}

impl Region {
    fn new(max_tiles: usize) -> Self {
        Self {
            tiles: Vec::new(),
            index: HashMap::new(),
            last: None,
            max_tiles,
        }
    }

    fn slot(&mut self, id: TileId) -> Option<usize> {
        if let Some((last_id, slot)) = self.last
            && last_id == id
        {
            return Some(slot);
        }
        let slot = self.index.get(&id).copied()?;
        self.last = Some((id, slot));
        Some(slot)
    }

    fn slot_or_insert(&mut self, id: TileId) -> Result<usize, FillError> {
        if let Some(slot) = self.slot(id) {
            return Ok(slot);
        }
        if self.tiles.len() >= self.max_tiles {
            return Err(FillError::TooLarge {
                max: self.max_tiles,
            });
        }
        let slot = self.tiles.len();
        self.tiles.push((id, vec![0_u64; WORDS].into_boxed_slice()));
        self.index.insert(id, slot);
        self.last = Some((id, slot));
        Ok(slot)
    }

    fn contains(&mut self, x: u32, y: u32) -> bool {
        let (id, lx, ly) = split(x, y);
        let Some(slot) = self.slot(id) else {
            return false;
        };
        let bit = ly * TILE_SIDE + lx;
        self.tiles
            .get(slot)
            .and_then(|(_, words)| words.get(bit / 64))
            .is_some_and(|word| word & (1 << (bit % 64)) != 0)
    }

    /// Marks `x0..=x1` of row `y`.
    fn set_span(&mut self, y: u32, x0: u32, x1: u32) -> Result<(), FillError> {
        let mut x = x0;
        while x <= x1 {
            let (id, lx, ly) = split(x, y);
            let slot = self.slot_or_insert(id)?;
            let run_end = x1.min(id.x.saturating_mul(TILE).saturating_add(TILE - 1));
            let last_lx = lx + (run_end - x) as usize;
            if let Some((_, words)) = self.tiles.get_mut(slot) {
                for l in lx..=last_lx {
                    let bit = ly * TILE_SIDE + l;
                    if let Some(word) = words.get_mut(bit / 64) {
                        *word |= 1 << (bit % 64);
                    }
                }
            }
            let Some(next) = run_end.checked_add(1) else {
                break;
            };
            x = next;
        }
        Ok(())
    }
}

/// What the Paint Bucket was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloodSpec {
    /// The clicked pixel, layer-local.
    pub seed: (u32, u32),
    /// 0..=255, 8-bit-equivalent units, against the clicked pixel.
    pub tolerance: u8,
    /// `true`: the 4-connected region around the seed; `false`: every
    /// similar pixel of the layer.
    pub contiguous: bool,
    /// The tile cap ([`MAX_FILL_TILES`] in the app; tests lower it).
    pub max_tiles: usize,
    /// The cap on existing tiles a non-contiguous fill reads
    /// ([`MAX_EXAMINED_TILES`] in the app; tests lower it).
    pub max_examined: usize,
}

/// Finds the matched region. `None` when the seed is outside `area`.
fn flood_region(
    plane: &mut Plane<'_>,
    area: (u32, u32),
    spec: &FloodSpec,
) -> Result<Option<Region>, FillError> {
    let (width, height) = area;
    let (sx, sy) = spec.seed;
    if sx >= width || sy >= height {
        return Ok(None);
    }
    let seed = plane.quantised(sx, sy)?;
    let mut region = Region::new(spec.max_tiles);
    if spec.contiguous {
        flood_contiguous(plane, &mut region, area, spec, seed)?;
    } else {
        flood_everywhere(plane, &mut region, area, spec, seed)?;
    }
    Ok(Some(region))
}

fn flood_contiguous(
    plane: &mut Plane<'_>,
    region: &mut Region,
    area: (u32, u32),
    spec: &FloodSpec,
    seed: Quantised,
) -> Result<(), FillError> {
    let (width, height) = area;
    let mut stack = vec![spec.seed];
    while let Some((x, y)) = stack.pop() {
        if region.contains(x, y) {
            continue;
        }
        if !within_tolerance(seed, plane.quantised(x, y)?, spec.tolerance) {
            continue;
        }
        let mut x0 = x;
        while x0 > 0 && within_tolerance(seed, plane.quantised(x0 - 1, y)?, spec.tolerance) {
            x0 -= 1;
        }
        let mut x1 = x;
        while let Some(next) = x1.checked_add(1).filter(|n| *n < width)
            && within_tolerance(seed, plane.quantised(next, y)?, spec.tolerance)
        {
            x1 = next;
        }
        region.set_span(y, x0, x1)?;
        for ny in [y.checked_sub(1), y.checked_add(1).filter(|v| *v < height)]
            .into_iter()
            .flatten()
        {
            let mut cx = x0;
            while cx <= x1 {
                let open = !region.contains(cx, ny)
                    && within_tolerance(seed, plane.quantised(cx, ny)?, spec.tolerance);
                if open {
                    stack.push((cx, ny));
                    // Skip the rest of this run: one seed per run.
                    cx = cx.saturating_add(1);
                    while cx <= x1
                        && !region.contains(cx, ny)
                        && within_tolerance(seed, plane.quantised(cx, ny)?, spec.tolerance)
                    {
                        cx = cx.saturating_add(1);
                    }
                } else {
                    cx = cx.saturating_add(1);
                }
            }
        }
    }
    Ok(())
}

fn flood_everywhere(
    plane: &mut Plane<'_>,
    region: &mut Region,
    area: (u32, u32),
    spec: &FloodSpec,
    seed: Quantised,
) -> Result<(), FillError> {
    let (width, height) = area;
    let blank_matches = within_tolerance(seed, [0; CHANNELS], spec.tolerance);
    let tiles_x = width.div_ceil(TILE);
    let tiles_y = height.div_ceil(TILE);
    let mut examined = 0_usize;
    for ty in 0..tiles_y {
        let y_start = ty.saturating_mul(TILE);
        let y_end = ty.saturating_add(1).saturating_mul(TILE).min(height);
        for tx in 0..tiles_x {
            let id = TileId { x: tx, y: ty };
            let x_start = tx.saturating_mul(TILE);
            let x_end = tx.saturating_add(1).saturating_mul(TILE).min(width);
            let last_x = x_end.saturating_sub(1);
            if !plane.store.contains_tile(plane.surface, id) {
                // A tile nobody wrote is fully transparent all over.
                if blank_matches {
                    for y in y_start..y_end {
                        region.set_span(y, x_start, last_x)?;
                    }
                }
                continue;
            }
            // Reading a held tile costs time even when nothing in it
            // matches, so those are bounded too, before the read.
            examined += 1;
            if examined > spec.max_examined {
                return Err(FillError::TooLarge {
                    max: spec.max_examined,
                });
            }
            for y in y_start..y_end {
                let mut run: Option<u32> = None;
                for x in x_start..x_end {
                    let hit = within_tolerance(seed, plane.quantised(x, y)?, spec.tolerance);
                    match (hit, run) {
                        (true, None) => run = Some(x),
                        (false, Some(start)) => {
                            region.set_span(y, start, x.saturating_sub(1))?;
                            run = None;
                        }
                        _ => {}
                    }
                }
                if let Some(start) = run {
                    region.set_span(y, start, last_x)?;
                }
            }
        }
    }
    Ok(())
}

/// The coverage to consult texel by texel in the `width x height` box at
/// tile-local `(x, y)` of the tile whose layer-local origin is
/// `origin`; `None` when the box is wholly uncovered (skip the tile), and
/// `Some(None)` when it is wholly covered or there is no selection.
#[allow(clippy::option_option)]
fn tile_coverage_of(
    coverage: Option<&dyn Coverage>,
    origin: (u32, u32),
    tile_box: (u32, u32, u32, u32),
) -> Option<Option<&dyn Coverage>> {
    let (x, y, width, height) = tile_box;
    let rect = Rect {
        x: i64::from(origin.0) + i64::from(x),
        y: i64::from(origin.1) + i64::from(y),
        width,
        height,
    };
    match tile_cover(coverage, rect) {
        TileCover::Skip => None,
        TileCover::All => Some(None),
        TileCover::Each(cov) => Some(Some(cov)),
    }
}

/// `fresh` blended over `current` by the coverage at the layer-local
/// texel (`origin` plus tile-local `at`), or `fresh` as is without one.
fn blended_by(
    tile_cov: Option<&dyn Coverage>,
    origin: (u32, u32),
    at: (usize, usize),
    current: &[f16],
    fresh: Texel,
) -> Option<Texel> {
    let Some(cov) = tile_cov else {
        return Some(fresh);
    };
    let x = i64::from(origin.0) + i64::try_from(at.0).unwrap_or(i64::MAX);
    let y = i64::from(origin.1) + i64::try_from(at.1).unwrap_or(i64::MAX);
    apply_coverage(current, fresh, covered(cov, x, y))
}

/// The texel a bucket fill writes over `current`, or `None` to leave it.
fn bucket_texel(mode: FillMode, colour: [f32; 3], current: &[f16]) -> Option<Texel> {
    let [red, green, blue] = colour;
    match mode {
        FillMode::Normal => Some([
            f16::from_f32(red),
            f16::from_f32(green),
            f16::from_f32(blue),
            f16::ONE,
        ]),
        FillMode::PreserveAlpha => {
            let alpha = current.get(3).copied()?;
            let a = alpha.to_f32();
            if a <= 0.0 {
                return None;
            }
            Some([
                f16::from_f32(red * a),
                f16::from_f32(green * a),
                f16::from_f32(blue * a),
                alpha,
            ])
        }
    }
}

fn same_bits(current: &[f16], fresh: Texel) -> bool {
    current.len() == fresh.len()
        && current
            .iter()
            .zip(fresh.iter())
            .all(|(a, b)| a.to_bits() == b.to_bits())
}

/// Chooses the texel to write over `current` at tile-local `(x, y)`.
type TexelFor<'a> = dyn FnMut(usize, usize, &[f16]) -> Option<Texel> + 'a;

/// Writes the texels `texel_for` chooses into one tile, capturing the
/// tile's old content first (once, before its first changed texel) and
/// marking the changed rectangle dirty. `pixels` yields tile-local `(x, y)`.
/// Returns whether anything changed.
///
/// **Plan first, write second (0.209.1).** The changes are worked out
/// against a read-only view of the tile (a tile the store does not hold is
/// all zeros and is *not* created to look at it); only when at least one
/// texel really differs is the tile acquired for writing
/// (`TileStore::get_mut`, which creates a missing tile and bumps its
/// content version). A fill that ends up changing nothing in a tile (a
/// transparency-locked fill over transparent pixels, opacity 0, an
/// already-filled region) therefore creates no tile, bumps no version and
/// captures nothing.
#[allow(clippy::cast_possible_wrap)]
fn write_tile(
    store: &mut TileStore,
    surface: SurfaceId,
    id: TileId,
    pixels: &mut dyn Iterator<Item = (usize, usize)>,
    texel_for: &mut TexelFor<'_>,
    snapshot: &mut Option<&mut StrokeSnapshot>,
) -> Result<bool, TileError> {
    let mut changes: Vec<(usize, usize, Texel)> = Vec::new();
    {
        let blank;
        let current: &[f16] = if store.contains_tile(surface, id) {
            store.get(surface, id)?.texels()
        } else {
            blank = vec![f16::ZERO; SAMPLES];
            &blank
        };
        for (lx, ly) in pixels {
            let index = (ly * TILE_SIDE + lx) * CHANNELS;
            let Some(old) = current.get(index..index + CHANNELS) else {
                continue;
            };
            let Some(fresh) = texel_for(lx, ly, old) else {
                continue;
            };
            if !same_bits(old, fresh) {
                changes.push((lx, ly, fresh));
            }
        }
    }
    let Some(&(fx, fy, _)) = changes.first() else {
        return Ok(false);
    };
    let tile = store.get_mut(surface, id)?;
    let texels = tile.texels_mut();
    if let Some(stroke) = snapshot.as_deref_mut()
        && !stroke.record_content(id, texels)
    {
        return Err(TileError::MalformedTile {
            surface,
            id,
            samples: texels.len(),
            expected: SAMPLES,
        });
    }
    let mut bounds = (fx, fy, fx, fy);
    for (lx, ly, fresh) in changes {
        let index = (ly * TILE_SIDE + lx) * CHANNELS;
        for (offset, value) in fresh.iter().enumerate() {
            if let Some(sample) = texels.get_mut(index + offset) {
                *sample = *value;
            }
        }
        bounds = (
            bounds.0.min(lx),
            bounds.1.min(ly),
            bounds.2.max(lx),
            bounds.3.max(ly),
        );
    }
    let (x0, y0, x1, y1) = bounds;
    tile.mark_dirty(Rect {
        x: x0 as i64,
        y: y0 as i64,
        width: (x1 - x0 + 1) as u32,
        height: (y1 - y0 + 1) as u32,
    });
    Ok(true)
}

/// The region the Magic Wand matched (0.214.0): a binary grid over the
/// matched pixels' bounding box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WandRegion {
    /// The tight bounding box of the matched pixels, layer-local.
    pub bounds: Rect,
    /// One byte per texel of `bounds`, row-major: `255` matched, `0` not.
    pub data: Vec<u8>,
}

/// Finds the pixels of the layer `area` (its `(width, height)`) similar to
/// the seed of `spec`, exactly as the Paint Bucket would fill them, and
/// returns them as a [`WandRegion`] without writing anything: the Magic
/// Wand (0.214.0). `None` when the seed is outside `area`. The same
/// similarity rule, tolerance semantics and caps as [`flood_fill`]
/// (`spec.max_tiles` matched tiles, `spec.max_examined` examined ones); in
/// addition the matched pixels' bounding box must not exceed
/// `spec.max_tiles` tiles of area (a sparse non-contiguous match can
/// otherwise span a huge box).
///
/// # Errors
///
/// [`FillError::TooLarge`] over a cap; [`FillError::Tile`] if a source
/// tile cannot be read.
pub fn wand_region(
    store: &mut TileStore,
    surface: SurfaceId,
    area: (u32, u32),
    spec: &FloodSpec,
) -> Result<Option<WandRegion>, FillError> {
    let mut plane = Plane::new(store, surface);
    let Some(region) = flood_region(&mut plane, area, spec)? else {
        return Ok(None);
    };
    // The tight box of the set bits, tile by tile.
    let mut lo = (u32::MAX, u32::MAX);
    let mut hi = (0_u32, 0_u32);
    let mut any = false;
    for (id, words) in &region.tiles {
        let (ox, oy) = (id.x.saturating_mul(TILE), id.y.saturating_mul(TILE));
        for (w, word) in words.iter().enumerate() {
            if *word == 0 {
                continue;
            }
            // Bit `b` of the tile is texel (b % SIDE, b / SIDE).
            let first = w * 64 + word.trailing_zeros() as usize;
            let last = w * 64 + 63 - word.leading_zeros() as usize;
            for bit in [first, last] {
                let x = ox.saturating_add(u32::try_from(bit % TILE_SIDE).unwrap_or(0));
                let y = oy.saturating_add(u32::try_from(bit / TILE_SIDE).unwrap_or(0));
                lo = (lo.0.min(x), lo.1.min(y));
                hi = (hi.0.max(x), hi.1.max(y));
            }
            // The row bounds above come from the lowest and highest set
            // bit; the column bounds need every set bit of the word.
            let mut rest = *word;
            while rest != 0 {
                let bit = w * 64 + rest.trailing_zeros() as usize;
                let x = ox.saturating_add(u32::try_from(bit % TILE_SIDE).unwrap_or(0));
                lo.0 = lo.0.min(x);
                hi.0 = hi.0.max(x);
                rest &= rest - 1;
            }
            any = true;
        }
    }
    if !any {
        return Ok(None);
    }
    let (bw, bh) = (hi.0 - lo.0 + 1, hi.1 - lo.1 + 1);
    let max_area = (spec.max_tiles as u64).saturating_mul(u64::from(TILE) * u64::from(TILE));
    if u64::from(bw) * u64::from(bh) > max_area {
        return Err(FillError::TooLarge {
            max: spec.max_tiles,
        });
    }
    let mut data = vec![0_u8; (bw as usize).saturating_mul(bh as usize)];
    for (id, words) in &region.tiles {
        let (ox, oy) = (id.x.saturating_mul(TILE), id.y.saturating_mul(TILE));
        for (w, word) in words.iter().enumerate() {
            let mut rest = *word;
            while rest != 0 {
                let bit = w * 64 + rest.trailing_zeros() as usize;
                rest &= rest - 1;
                let x = ox.saturating_add(u32::try_from(bit % TILE_SIDE).unwrap_or(0));
                let y = oy.saturating_add(u32::try_from(bit / TILE_SIDE).unwrap_or(0));
                let at = (y - lo.1) as usize * bw as usize + (x - lo.0) as usize;
                if let Some(slot) = data.get_mut(at) {
                    *slot = 255;
                }
            }
        }
    }
    Ok(Some(WandRegion {
        bounds: Rect {
            x: i64::from(lo.0),
            y: i64::from(lo.1),
            width: bw,
            height: bh,
        },
        data,
    }))
}

/// Fills the region `spec` selects with `colour` (the Paint Bucket).
///
/// `area` is the layer's `(width, height)`; the seed must lie inside it.
/// The region is found completely before any texel is written, so a
/// [`FillError`] leaves the layer exactly as it was. A fill that would
/// change no texel (the region is already `colour`) captures nothing and
/// paints no tile.
///
/// # Errors
///
/// [`FillError::TooLarge`] when the region touches more than
/// `spec.max_tiles` tiles; [`FillError::Tile`] if a source tile cannot be
/// read.
pub fn flood_fill(
    store: &mut TileStore,
    surface: SurfaceId,
    area: (u32, u32),
    spec: &FloodSpec,
    colour: [f32; 3],
    mode: FillMode,
    snapshot: Option<&mut StrokeSnapshot>,
) -> Result<FillOutcome, FillError> {
    flood_fill_covered(store, surface, area, spec, colour, mode, snapshot, None)
}

/// [`flood_fill`] restricted to a selection (0.211.0): the region is found
/// exactly as before (a contiguous region may run through unselected
/// pixels), then each of its texels is blended in by `coverage`
/// (`old + (fill - old) * coverage`, see [`Coverage`]) and a tile with no
/// covered texel is neither read for writing nor created. A seed whose own
/// coverage is `0` does nothing at all and reads nothing. The tile caps
/// count the matched region, not the covered part of it. `None` is exactly
/// [`flood_fill`]; coverage `1` writes the same bits.
///
/// # Errors
///
/// As [`flood_fill`].
#[allow(clippy::too_many_arguments)]
pub fn flood_fill_covered(
    store: &mut TileStore,
    surface: SurfaceId,
    area: (u32, u32),
    spec: &FloodSpec,
    colour: [f32; 3],
    mode: FillMode,
    mut snapshot: Option<&mut StrokeSnapshot>,
    coverage: Option<&dyn Coverage>,
) -> Result<FillOutcome, FillError> {
    let mut outcome = FillOutcome::default();
    if let Some(cov) = coverage
        && covered(cov, i64::from(spec.seed.0), i64::from(spec.seed.1)) <= 0.0
    {
        // The clicked pixel is outside the selection: Photoshop does
        // nothing, and so nothing is read.
        return Ok(outcome);
    }
    let region = {
        let mut plane = Plane::new(store, surface);
        flood_region(&mut plane, area, spec)?
    };
    let Some(region) = region else {
        return Ok(outcome);
    };
    for (id, words) in &region.tiles {
        let origin = (id.x.saturating_mul(TILE), id.y.saturating_mul(TILE));
        let Some(tile_cov) = tile_coverage_of(coverage, origin, (0, 0, TILE, TILE)) else {
            continue;
        };
        let mut bits = words
            .iter()
            .enumerate()
            .flat_map(|(w, word)| {
                let word = *word;
                (0..64_usize)
                    .filter(move |b| word & (1_u64 << b) != 0)
                    .map(move |b| w * 64 + b)
            })
            .map(|bit| (bit % TILE_SIDE, bit / TILE_SIDE));
        let mut texel_for = |lx: usize, ly: usize, current: &[f16]| {
            let fresh = bucket_texel(mode, colour, current)?;
            blended_by(tile_cov, origin, (lx, ly), current, fresh)
        };
        match write_tile(
            store,
            surface,
            *id,
            &mut bits,
            &mut texel_for,
            &mut snapshot,
        ) {
            Ok(true) => outcome.painted.push(*id),
            Ok(false) => {}
            Err(err) => outcome.failed.push((*id, err)),
        }
    }
    Ok(outcome)
}

/// The two gradient shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientKind {
    /// `t` grows along the start-to-end line, clamped to `[0, 1]`.
    Linear,
    /// `t` is the distance from the start over the start-to-end length,
    /// clamped to `[0, 1]`.
    Radial,
}

/// What the Gradient was asked to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientSpec {
    /// Linear or radial.
    pub kind: GradientKind,
    /// The drag's start, layer-local (document point minus the layer's
    /// origin).
    pub start: (f32, f32),
    /// The drag's end, layer-local.
    pub end: (f32, f32),
    /// The colour at `t = 0` (straight encoded sRGB).
    pub from: [f32; 3],
    /// The colour at `t = 1`.
    pub to: [f32; 3],
    /// 0..=1: how much of the gradient replaces what is there.
    pub opacity: f32,
    /// The tile cap ([`MAX_FILL_TILES`] in the app; tests lower it).
    pub max_tiles: usize,
}

impl GradientSpec {
    /// The gradient parameter at layer-local point `(px, py)`, in `[0, 1]`
    /// (`0` at/behind the start, `1` at/beyond the end). `None` for a
    /// degenerate (zero-length or non-finite) drag.
    #[must_use]
    pub fn parameter(&self, px: f32, py: f32) -> Option<f32> {
        let (dx, dy) = (self.end.0 - self.start.0, self.end.1 - self.start.1);
        let length_sq = dx * dx + dy * dy;
        if !(length_sq.is_finite() && length_sq > 1e-6) {
            return None;
        }
        let (rx, ry) = (px - self.start.0, py - self.start.1);
        let t = match self.kind {
            GradientKind::Linear => (rx * dx + ry * dy) / length_sq,
            GradientKind::Radial => rx.hypot(ry) / length_sq.sqrt(),
        };
        Some(if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        })
    }

    /// The colour at parameter `t`: a straight interpolation of the two
    /// colours in encoded sRGB.
    #[must_use]
    pub fn colour_at(&self, t: f32) -> [f32; 3] {
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        [
            lerp(self.from[0], self.to[0]),
            lerp(self.from[1], self.to[1]),
            lerp(self.from[2], self.to[2]),
        ]
    }

    /// Whether the drag has a direction at all.
    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        self.parameter(self.start.0, self.start.1).is_none()
    }
}

/// The texel a gradient writes over `current` at layer-local pixel
/// `(x, y)`, or `None` to leave it.
fn gradient_texel(
    spec: &GradientSpec,
    mode: FillMode,
    x: u32,
    y: u32,
    current: &[f16],
) -> Option<Texel> {
    #[allow(clippy::cast_precision_loss)]
    let t = spec.parameter(x as f32 + 0.5, y as f32 + 0.5)?;
    let [red, green, blue] = spec.colour_at(t);
    let opacity = spec.opacity.clamp(0.0, 1.0);
    let old = |c: usize| current.get(c).map_or(0.0, |v| v.to_f32());
    match mode {
        FillMode::Normal => {
            // Premultiplied "source over" of an opaque colour at `opacity`.
            let keep = 1.0 - opacity;
            Some([
                f16::from_f32(red * opacity + old(0) * keep),
                f16::from_f32(green * opacity + old(1) * keep),
                f16::from_f32(blue * opacity + old(2) * keep),
                f16::from_f32(opacity + old(3) * keep),
            ])
        }
        FillMode::PreserveAlpha => {
            let alpha = current.get(3).copied()?;
            let a = alpha.to_f32();
            if a <= 0.0 {
                return None;
            }
            let keep = 1.0 - opacity;
            Some([
                f16::from_f32(old(0) * keep + red * a * opacity),
                f16::from_f32(old(1) * keep + green * a * opacity),
                f16::from_f32(old(2) * keep + blue * a * opacity),
                alpha,
            ])
        }
    }
}

/// Paints `spec`'s gradient over the layer-local `area` (`x0, y0, x1, y1`,
/// exclusive ends). A degenerate drag, an empty area or a non-finite
/// opacity paints nothing.
///
/// # Errors
///
/// [`FillError::TooLarge`] — before any write — when `area` spans more
/// than `spec.max_tiles` tiles.
pub fn gradient_fill(
    store: &mut TileStore,
    surface: SurfaceId,
    area: (u32, u32, u32, u32),
    spec: &GradientSpec,
    mode: FillMode,
    snapshot: Option<&mut StrokeSnapshot>,
) -> Result<FillOutcome, FillError> {
    gradient_fill_covered(store, surface, area, spec, mode, snapshot, None)
}

/// [`gradient_fill`] restricted to a selection (0.211.0): the gradient is
/// evaluated as before (its start and end do not move) and each texel is
/// blended in by `coverage`. The area is first clipped to
/// [`Coverage::bounds`], so only the tiles the selection can reach count
/// toward `spec.max_tiles` and are visited; a tile with no covered texel
/// is neither read nor created. `None` is exactly [`gradient_fill`].
///
/// # Errors
///
/// As [`gradient_fill`].
pub fn gradient_fill_covered(
    store: &mut TileStore,
    surface: SurfaceId,
    area: (u32, u32, u32, u32),
    spec: &GradientSpec,
    mode: FillMode,
    mut snapshot: Option<&mut StrokeSnapshot>,
    coverage: Option<&dyn Coverage>,
) -> Result<FillOutcome, FillError> {
    let mut outcome = FillOutcome::default();
    let (mut x0, mut y0, mut x1, mut y1) = area;
    if let Some(bounds) = coverage.and_then(Coverage::bounds) {
        let clip = |v: i64| u32::try_from(v.clamp(0, i64::from(u32::MAX))).unwrap_or(u32::MAX);
        x0 = x0.max(clip(bounds.x));
        y0 = y0.max(clip(bounds.y));
        x1 = x1.min(clip(bounds.right()));
        y1 = y1.min(clip(bounds.bottom()));
    }
    if x0 >= x1
        || y0 >= y1
        || spec.is_degenerate()
        || !spec.opacity.is_finite()
        // Opacity zero changes nothing: no tile is read or created.
        || spec.opacity <= 0.0
    {
        return Ok(outcome);
    }
    let (tx0, tx1) = (x0 / TILE, (x1 - 1) / TILE);
    let (ty0, ty1) = (y0 / TILE, (y1 - 1) / TILE);
    let tiles = u64::from(tx1 - tx0 + 1) * u64::from(ty1 - ty0 + 1);
    if tiles > spec.max_tiles as u64 {
        return Err(FillError::TooLarge {
            max: spec.max_tiles,
        });
    }
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let id = TileId { x: tx, y: ty };
            let (origin_x, origin_y) = (tx.saturating_mul(TILE), ty.saturating_mul(TILE));
            let lx0 = x0.max(origin_x) - origin_x;
            let lx1 = x1.min(origin_x.saturating_add(TILE)) - origin_x;
            let ly0 = y0.max(origin_y) - origin_y;
            let ly1 = y1.min(origin_y.saturating_add(TILE)) - origin_y;
            let Some(tile_cov) = tile_coverage_of(
                coverage,
                (origin_x, origin_y),
                (lx0, ly0, lx1 - lx0, ly1 - ly0),
            ) else {
                continue;
            };
            let mut pixels =
                (ly0..ly1).flat_map(move |ly| (lx0..lx1).map(move |lx| (lx as usize, ly as usize)));
            let mut texel_for = |lx: usize, ly: usize, current: &[f16]| {
                let fresh = gradient_texel(
                    spec,
                    mode,
                    origin_x.saturating_add(u32::try_from(lx).unwrap_or(0)),
                    origin_y.saturating_add(u32::try_from(ly).unwrap_or(0)),
                    current,
                )?;
                blended_by(tile_cov, (origin_x, origin_y), (lx, ly), current, fresh)
            };
            match write_tile(
                store,
                surface,
                id,
                &mut pixels,
                &mut texel_for,
                &mut snapshot,
            ) {
                Ok(true) => outcome.painted.push(id),
                Ok(false) => {}
                Err(err) => outcome.failed.push((id, err)),
            }
        }
    }
    Ok(outcome)
}

/// What [`fill_with_coverage`] paints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeFillSpec {
    /// The straight encoded sRGB colour.
    pub colour: [f32; 3],
    /// The opacity the whole fill is capped at, `0..=1` (a texel's amount is
    /// the shape's coverage there times this).
    pub opacity: f32,
    /// The tile cap, as [`FloodSpec::max_tiles`].
    pub max_tiles: usize,
}

/// The texel `colour` makes over `current` at `amount` (`0..=1`): at `1`
/// exactly what [`bucket_texel`] gives, below it source-over of the colour
/// (opaque) at that alpha; under [`FillMode::PreserveAlpha`] the colour is
/// moved toward `colour * alpha` by `amount` and the alpha is kept.
fn solid_texel(mode: FillMode, colour: [f32; 3], amount: f32, current: &[f16]) -> Option<Texel> {
    if amount >= 1.0 {
        return bucket_texel(mode, colour, current);
    }
    if amount <= 0.0 {
        return None;
    }
    let old = |c: usize| current.get(c).map_or(0.0, |v| v.to_f32());
    let keep = 1.0 - amount;
    let [red, green, blue] = colour;
    match mode {
        FillMode::Normal => Some([
            f16::from_f32(red * amount + old(0) * keep),
            f16::from_f32(green * amount + old(1) * keep),
            f16::from_f32(blue * amount + old(2) * keep),
            f16::from_f32(amount + old(3) * keep),
        ]),
        FillMode::PreserveAlpha => {
            let alpha = current.get(3).copied()?;
            let a = alpha.to_f32();
            if a <= 0.0 {
                return None;
            }
            Some([
                f16::from_f32(old(0) * keep + red * a * amount),
                f16::from_f32(old(1) * keep + green * a * amount),
                f16::from_f32(old(2) * keep + blue * a * amount),
                alpha,
            ])
        }
    }
}

/// Fills a **shape given as a coverage** with one colour (Fill Path,
/// 0.234.0): every texel of the layer-local `area` is painted by
/// `shape.at(x, y) * spec.opacity` of the way from what it holds to the
/// colour (source-over of the opaque colour at that alpha), then blended in
/// by the optional `selection` like every other fill. At shape coverage `1`
/// and opacity `1` a texel is exactly the Paint Bucket's
/// ([`flood_fill_covered`]); the transparency lock ([`FillMode::PreserveAlpha`])
/// recolours without ever changing alpha.
///
/// The area is first clipped to the shape's and the selection's bounds, so
/// only tiles they can reach count toward `spec.max_tiles` and are read; a
/// tile with no covered texel is neither read for writing nor created, and a
/// fill that changes nothing captures nothing.
///
/// # Errors
///
/// [`FillError::TooLarge`] — before any write — when the clipped area spans
/// more than `spec.max_tiles` tiles.
#[allow(clippy::too_many_arguments)]
pub fn fill_with_coverage(
    store: &mut TileStore,
    surface: SurfaceId,
    area: (u32, u32, u32, u32),
    shape: &dyn Coverage,
    spec: &ShapeFillSpec,
    mode: FillMode,
    mut snapshot: Option<&mut StrokeSnapshot>,
    selection: Option<&dyn Coverage>,
) -> Result<FillOutcome, FillError> {
    let mut outcome = FillOutcome::default();
    let (mut x0, mut y0, mut x1, mut y1) = area;
    let clip = |v: i64| u32::try_from(v.clamp(0, i64::from(u32::MAX))).unwrap_or(u32::MAX);
    for bounds in [shape.bounds(), selection.and_then(Coverage::bounds)]
        .into_iter()
        .flatten()
    {
        x0 = x0.max(clip(bounds.x));
        y0 = y0.max(clip(bounds.y));
        x1 = x1.min(clip(bounds.right()));
        y1 = y1.min(clip(bounds.bottom()));
    }
    if x0 >= x1 || y0 >= y1 || !spec.opacity.is_finite() || spec.opacity <= 0.0 {
        return Ok(outcome);
    }
    let opacity = spec.opacity.min(1.0);
    let (tx0, tx1) = (x0 / TILE, (x1 - 1) / TILE);
    let (ty0, ty1) = (y0 / TILE, (y1 - 1) / TILE);
    let tiles = u64::from(tx1 - tx0 + 1) * u64::from(ty1 - ty0 + 1);
    if tiles > spec.max_tiles as u64 {
        return Err(FillError::TooLarge {
            max: spec.max_tiles,
        });
    }
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let id = TileId { x: tx, y: ty };
            let (origin_x, origin_y) = (tx.saturating_mul(TILE), ty.saturating_mul(TILE));
            let lx0 = x0.max(origin_x) - origin_x;
            let lx1 = x1.min(origin_x.saturating_add(TILE)) - origin_x;
            let ly0 = y0.max(origin_y) - origin_y;
            let ly1 = y1.min(origin_y.saturating_add(TILE)) - origin_y;
            let tile_box = (lx0, ly0, lx1 - lx0, ly1 - ly0);
            let Some(shape_cov) = tile_coverage_of(Some(shape), (origin_x, origin_y), tile_box)
            else {
                continue;
            };
            let Some(sel_cov) = tile_coverage_of(selection, (origin_x, origin_y), tile_box) else {
                continue;
            };
            let mut pixels =
                (ly0..ly1).flat_map(move |ly| (lx0..lx1).map(move |lx| (lx as usize, ly as usize)));
            let mut texel_for = |lx: usize, ly: usize, current: &[f16]| {
                let x = i64::from(origin_x) + i64::try_from(lx).unwrap_or(i64::MAX);
                let y = i64::from(origin_y) + i64::try_from(ly).unwrap_or(i64::MAX);
                let share = shape_cov.map_or(1.0, |cov| covered(cov, x, y));
                let fresh = solid_texel(mode, spec.colour, share * opacity, current)?;
                blended_by(sel_cov, (origin_x, origin_y), (lx, ly), current, fresh)
            };
            match write_tile(
                store,
                surface,
                id,
                &mut pixels,
                &mut texel_for,
                &mut snapshot,
            ) {
                Ok(true) => outcome.painted.push(id),
                Ok(false) => {}
                Err(err) => outcome.failed.push((id, err)),
            }
        }
    }
    Ok(outcome)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use std::num::NonZeroUsize;

    use super::*;
    use crate::test_support::surface;

    const RED: [f32; 3] = [1.0, 0.0, 0.0];

    fn store() -> (tempfile::TempDir, TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("tempdir: {err}"),
        };
        let Some(budget) = NonZeroUsize::new(64) else {
            unreachable!("non-zero");
        };
        match TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => (dir, store),
            Err(err) => unreachable!("store: {err:?}"),
        }
    }

    fn put(store: &mut TileStore, x: u32, y: u32, texel: Texel) {
        let (id, lx, ly) = split(x, y);
        let Ok(tile) = store.get_mut(surface(), id) else {
            unreachable!("tile");
        };
        let index = (ly * TILE_SIDE + lx) * CHANNELS;
        for (offset, value) in texel.iter().enumerate() {
            if let Some(sample) = tile.texels_mut().get_mut(index + offset) {
                *sample = *value;
            }
        }
    }

    fn get(store: &mut TileStore, x: u32, y: u32) -> [f32; 4] {
        let (id, lx, ly) = split(x, y);
        if !store.contains_tile(surface(), id) {
            return [0.0; 4];
        }
        let Ok(tile) = store.get(surface(), id) else {
            unreachable!("tile");
        };
        let index = (ly * TILE_SIDE + lx) * CHANNELS;
        let at = |o: usize| {
            tile.texels()
                .get(index + o)
                .map_or(f32::NAN, |v| v.to_f32())
        };
        [at(0), at(1), at(2), at(3)]
    }

    fn grey(v: f32) -> Texel {
        let h = f16::from_f32(v);
        [h, h, h, f16::ONE]
    }

    fn fill_rect(store: &mut TileStore, x0: u32, y0: u32, x1: u32, y1: u32, texel: Texel) {
        for y in y0..y1 {
            for x in x0..x1 {
                put(store, x, y, texel);
            }
        }
    }

    fn spec(seed: (u32, u32), tolerance: u8, contiguous: bool) -> FloodSpec {
        FloodSpec {
            seed,
            tolerance,
            contiguous,
            max_tiles: MAX_FILL_TILES,
            max_examined: MAX_EXAMINED_TILES,
        }
    }

    fn bucket(
        store: &mut TileStore,
        area: (u32, u32),
        spec: &FloodSpec,
        mode: FillMode,
    ) -> Result<(usize, StrokeSnapshot), FillError> {
        let mut snapshot = StrokeSnapshot::new(surface());
        let outcome = flood_fill(store, surface(), area, spec, RED, mode, Some(&mut snapshot))?;
        assert!(outcome.failed().is_empty());
        Ok((outcome.painted().len(), snapshot))
    }

    fn is_red(p: [f32; 4]) -> bool {
        p == [1.0, 0.0, 0.0, 1.0]
    }

    #[test]
    fn contiguous_fill_stops_at_a_wall_but_global_fill_crosses_it() {
        let (_dir, mut s) = store();
        // Opaque white 8 x 4, a black wall at x = 4, white either side.
        fill_rect(&mut s, 0, 0, 8, 4, grey(1.0));
        fill_rect(&mut s, 4, 0, 5, 4, grey(0.0));
        let Ok(_) = bucket(&mut s, (8, 4), &spec((1, 1), 0, true), FillMode::Normal) else {
            unreachable!()
        };
        assert!(is_red(get(&mut s, 0, 0)) && is_red(get(&mut s, 3, 3)));
        assert_eq!(get(&mut s, 4, 1)[0], 0.0, "the wall is untouched");
        assert_eq!(get(&mut s, 6, 1), [1.0; 4], "the far side is untouched");

        let (_dir, mut s) = store();
        fill_rect(&mut s, 0, 0, 8, 4, grey(1.0));
        fill_rect(&mut s, 4, 0, 5, 4, grey(0.0));
        let Ok(_) = bucket(&mut s, (8, 4), &spec((1, 1), 0, false), FillMode::Normal) else {
            unreachable!()
        };
        assert!(is_red(get(&mut s, 0, 0)) && is_red(get(&mut s, 6, 1)));
        assert_eq!(get(&mut s, 4, 1)[0], 0.0);
    }

    #[test]
    fn tolerance_boundary_is_inclusive_at_32_and_zero_is_exact() {
        // 8-bit-equivalent values: seed 100, neighbour 132 (diff 32) and
        // 133 (diff 33).
        let level = |q: f32| grey(q / 255.0);
        let make = || {
            let (dir, mut s) = store();
            put(&mut s, 0, 0, level(100.0));
            put(&mut s, 1, 0, level(132.0));
            put(&mut s, 2, 0, level(133.0));
            (dir, s)
        };
        let (_d, mut s) = make();
        let Ok(_) = bucket(&mut s, (3, 1), &spec((0, 0), 32, true), FillMode::Normal) else {
            unreachable!()
        };
        assert!(is_red(get(&mut s, 1, 0)), "diff 32 matches at tolerance 32");
        assert!(!is_red(get(&mut s, 2, 0)), "diff 33 does not");

        let (_d, mut s) = make();
        let Ok(_) = bucket(&mut s, (3, 1), &spec((0, 0), 31, true), FillMode::Normal) else {
            unreachable!()
        };
        assert!(!is_red(get(&mut s, 1, 0)), "diff 32 fails at tolerance 31");

        let (_d, mut s) = make();
        let Ok(_) = bucket(&mut s, (3, 1), &spec((0, 0), 0, true), FillMode::Normal) else {
            unreachable!()
        };
        assert!(is_red(get(&mut s, 0, 0)));
        assert!(!is_red(get(&mut s, 1, 0)), "tolerance 0 is exact");
    }

    #[test]
    fn tolerance_255_matches_everything_in_the_layer() {
        let (_dir, mut s) = store();
        put(&mut s, 0, 0, grey(0.0));
        put(&mut s, 1, 0, grey(1.0));
        put(
            &mut s,
            2,
            0,
            [f16::ONE, f16::ZERO, f16::ONE, f16::from_f32(0.5)],
        );
        let Ok(_) = bucket(&mut s, (3, 1), &spec((0, 0), 255, true), FillMode::Normal) else {
            unreachable!()
        };
        for x in 0..3 {
            assert!(is_red(get(&mut s, x, 0)), "x = {x}");
        }
    }

    #[test]
    fn alpha_is_part_of_the_comparison() {
        let (_dir, mut s) = store();
        // A transparent pixel next to an opaque one of the same RGB.
        put(&mut s, 0, 0, [f16::ONE, f16::ONE, f16::ONE, f16::ZERO]);
        put(&mut s, 1, 0, grey(1.0));
        let Ok(_) = bucket(&mut s, (2, 1), &spec((1, 0), 0, true), FillMode::Normal) else {
            unreachable!()
        };
        assert!(is_red(get(&mut s, 1, 0)));
        assert_eq!(get(&mut s, 0, 0)[3], 0.0, "transparent is not opaque white");
    }

    #[test]
    fn a_fully_transparent_seed_matches_unwritten_tiles_without_creating_them() {
        let (_dir, mut s) = store();
        let Ok((painted, _)) = bucket(&mut s, (300, 10), &spec((5, 5), 0, true), FillMode::Normal)
        else {
            unreachable!()
        };
        assert_eq!(painted, 2, "tiles (0,0) and (1,0)");
        assert!(is_red(get(&mut s, 0, 0)) && is_red(get(&mut s, 299, 9)));
        assert!(!s.contains_tile(surface(), TileId { x: 0, y: 1 }));
    }

    #[test]
    fn the_fill_crosses_tile_boundaries_and_stops_at_the_area_edge() {
        let (_dir, mut s) = store();
        fill_rect(&mut s, 250, 250, 262, 262, grey(1.0));
        let Ok((painted, _)) = bucket(
            &mut s,
            (262, 262),
            &spec((255, 255), 0, true),
            FillMode::Normal,
        ) else {
            unreachable!()
        };
        assert_eq!(painted, 4, "the square meets the corner of four tiles");
        assert!(is_red(get(&mut s, 250, 250)) && is_red(get(&mut s, 261, 261)));
        assert_eq!(get(&mut s, 262, 262), [0.0; 4]);
        // Area clip: the same square with the layer only 256 wide.
        let (_dir, mut s) = store();
        fill_rect(&mut s, 250, 250, 262, 262, grey(1.0));
        let Ok(_) = bucket(
            &mut s,
            (256, 256),
            &spec((255, 255), 0, true),
            FillMode::Normal,
        ) else {
            unreachable!()
        };
        assert!(is_red(get(&mut s, 255, 255)));
        assert_eq!(
            get(&mut s, 256, 255),
            [1.0; 4],
            "outside the area is untouched"
        );
    }

    #[test]
    fn a_hollow_shape_keeps_its_hole_and_a_one_pixel_corridor_is_followed() {
        let (_dir, mut s) = store();
        // A ring of black on transparent, 9 x 9, with a hole in the middle.
        fill_rect(&mut s, 0, 0, 9, 9, grey(0.0));
        fill_rect(&mut s, 3, 3, 6, 6, [f16::ZERO; 4]);
        // Click outside the ring on a transparent pixel (area bigger).
        let Ok(_) = bucket(&mut s, (12, 12), &spec((10, 10), 0, true), FillMode::Normal) else {
            unreachable!()
        };
        assert!(is_red(get(&mut s, 11, 11)));
        assert_eq!(get(&mut s, 4, 4)[3], 0.0, "the enclosed hole stays empty");
        assert_eq!(get(&mut s, 0, 0)[0], 0.0);

        // A winding one-pixel corridor of white through black.
        let (_dir, mut s) = store();
        fill_rect(&mut s, 0, 0, 7, 5, grey(0.0));
        for (x, y) in [
            (0, 0),
            (1, 0),
            (2, 0),
            (2, 1),
            (2, 2),
            (3, 2),
            (4, 2),
            (4, 3),
            (4, 4),
            (5, 4),
            (6, 4),
        ] {
            put(&mut s, x, y, grey(1.0));
        }
        let Ok(_) = bucket(&mut s, (7, 5), &spec((0, 0), 0, true), FillMode::Normal) else {
            unreachable!()
        };
        assert!(
            is_red(get(&mut s, 6, 4)),
            "the corridor's far end is reached"
        );
        assert_eq!(get(&mut s, 1, 1)[0], 0.0);
    }

    #[test]
    fn connectivity_is_four_not_eight() {
        let (_dir, mut s) = store();
        // Two white pixels touching only diagonally, on black.
        fill_rect(&mut s, 0, 0, 3, 3, grey(0.0));
        put(&mut s, 0, 0, grey(1.0));
        put(&mut s, 1, 1, grey(1.0));
        let Ok(_) = bucket(&mut s, (3, 3), &spec((0, 0), 0, true), FillMode::Normal) else {
            unreachable!()
        };
        assert!(is_red(get(&mut s, 0, 0)));
        assert!(
            !is_red(get(&mut s, 1, 1)),
            "a diagonal neighbour is not connected"
        );
    }

    #[test]
    fn a_huge_empty_layer_is_refused_with_nothing_written() {
        let (_dir, mut s) = store();
        let mut snapshot = StrokeSnapshot::new(surface());
        let result = flood_fill(
            &mut s,
            surface(),
            (30_000, 30_000),
            &FloodSpec {
                seed: (10, 10),
                tolerance: 0,
                contiguous: true,
                max_tiles: 8,
                max_examined: MAX_EXAMINED_TILES,
            },
            RED,
            FillMode::Normal,
            Some(&mut snapshot),
        );
        assert!(matches!(result, Err(FillError::TooLarge { max: 8 })));
        assert!(snapshot.is_empty());
        assert_eq!(s.resident_len(), 0, "no tile was created");
        // The non-contiguous mode refuses too, without walking the layer.
        let result = flood_fill(
            &mut s,
            surface(),
            (300_000, 300_000),
            &FloodSpec {
                seed: (10, 10),
                tolerance: 0,
                contiguous: false,
                max_tiles: 8,
                max_examined: MAX_EXAMINED_TILES,
            },
            RED,
            FillMode::Normal,
            None,
        );
        assert!(matches!(result, Err(FillError::TooLarge { .. })));
    }

    #[test]
    fn a_region_exactly_at_the_cap_is_allowed_and_one_more_tile_is_not() {
        let (_dir, mut s) = store();
        let ok = flood_fill(
            &mut s,
            surface(),
            (512, 256),
            &FloodSpec {
                seed: (0, 0),
                tolerance: 0,
                contiguous: true,
                max_tiles: 2,
                max_examined: MAX_EXAMINED_TILES,
            },
            RED,
            FillMode::Normal,
            None,
        );
        assert!(ok.is_ok());
        let (_dir, mut s) = store();
        let refused = flood_fill(
            &mut s,
            surface(),
            (768, 256),
            &FloodSpec {
                seed: (0, 0),
                tolerance: 0,
                contiguous: true,
                max_tiles: 2,
                max_examined: MAX_EXAMINED_TILES,
            },
            RED,
            FillMode::Normal,
            None,
        );
        assert!(matches!(refused, Err(FillError::TooLarge { max: 2 })));
    }

    #[test]
    fn a_fill_of_an_already_filled_region_changes_and_captures_nothing() {
        let (_dir, mut s) = store();
        fill_rect(
            &mut s,
            0,
            0,
            4,
            4,
            [f16::ONE, f16::ZERO, f16::ZERO, f16::ONE],
        );
        let Ok((painted, snapshot)) =
            bucket(&mut s, (4, 4), &spec((1, 1), 0, true), FillMode::Normal)
        else {
            unreachable!()
        };
        assert_eq!(painted, 0);
        assert!(snapshot.is_empty());
    }

    #[test]
    fn a_seed_outside_the_area_does_nothing() {
        let (_dir, mut s) = store();
        let Ok((painted, snapshot)) =
            bucket(&mut s, (4, 4), &spec((4, 0), 0, true), FillMode::Normal)
        else {
            unreachable!()
        };
        assert_eq!(painted, 0);
        assert!(snapshot.is_empty());
    }

    #[test]
    fn a_transparency_locked_fill_recolours_without_touching_alpha() {
        let (_dir, mut s) = store();
        let half = f16::from_f32(0.5);
        put(&mut s, 0, 0, [half, half, half, half]);
        put(&mut s, 1, 0, [half, half, half, half]);
        put(&mut s, 2, 0, [f16::ZERO; 4]);
        let Ok(_) = bucket(
            &mut s,
            (3, 1),
            &spec((0, 0), 255, true),
            FillMode::PreserveAlpha,
        ) else {
            unreachable!()
        };
        assert_eq!(
            get(&mut s, 0, 0),
            [0.5, 0.0, 0.0, 0.5],
            "red x alpha, alpha kept"
        );
        assert_eq!(get(&mut s, 2, 0), [0.0; 4], "transparent stays transparent");
    }

    #[test]
    fn a_transparency_locked_fill_leaves_stale_colour_under_zero_alpha_alone() {
        let (_dir, mut s) = store();
        // Fully transparent but with leftover RGB: a locked layer must
        // not be touched there (the unlocked fill would replace it).
        put(&mut s, 0, 0, [f16::ONE, f16::ONE, f16::ONE, f16::ZERO]);
        let Ok((painted, snapshot)) = bucket(
            &mut s,
            (1, 1),
            &spec((0, 0), 255, true),
            FillMode::PreserveAlpha,
        ) else {
            unreachable!()
        };
        assert_eq!(painted, 0);
        assert!(snapshot.is_empty());
        assert_eq!(get(&mut s, 0, 0), [1.0, 1.0, 1.0, 0.0]);
    }

    #[test]
    fn fills_that_change_nothing_create_no_tile_and_bump_no_version() {
        let (_dir, mut s) = store();
        // Transparency lock over an empty layer: nothing to recolour.
        let r = bucket(
            &mut s,
            (600, 300),
            &spec((5, 5), 255, true),
            FillMode::PreserveAlpha,
        );
        assert!(matches!(r, Ok((0, ref snap)) if snap.is_empty()));
        let spec_all = linear([0.0; 3], [1.0; 3]);
        let r = gradient(&mut s, (0, 0, 600, 300), &spec_all, FillMode::PreserveAlpha);
        assert!(matches!(r, Ok((0, ref snap)) if snap.is_empty()));
        // Opacity zero, on an empty layer and on a held tile.
        let mut zero = linear([0.0; 3], [1.0; 3]);
        zero.opacity = 0.0;
        let r = gradient(&mut s, (0, 0, 600, 300), &zero, FillMode::Normal);
        assert!(matches!(r, Ok((0, ref snap)) if snap.is_empty()));
        assert_eq!(s.resident_len(), 0, "no tile was created");
        for x in 0..3 {
            for y in 0..3 {
                assert!(!s.contains_tile(surface(), TileId { x, y }));
            }
        }
        // A held tile that ends unchanged keeps its content version.
        put(&mut s, 0, 0, grey(1.0));
        let id = TileId { x: 0, y: 0 };
        let version = s.content_version(surface(), id);
        let r = gradient(&mut s, (0, 0, 8, 8), &zero, FillMode::Normal);
        assert!(matches!(r, Ok((0, _))));
        assert_eq!(
            s.content_version(surface(), id),
            version,
            "opacity 0 touched nothing"
        );
        // A locked bucket over transparent pixels of a held tile: unchanged.
        let r = bucket(
            &mut s,
            (8, 8),
            &spec((200, 200), 0, true),
            FillMode::PreserveAlpha,
        );
        assert!(matches!(r, Ok((0, _))));
        assert_eq!(s.content_version(surface(), id), version);
        let before = s.content_version(surface(), id);
        let r = gradient(&mut s, (0, 0, 8, 8), &zero, FillMode::Normal);
        assert!(matches!(r, Ok((0, _))));
        assert_eq!(s.content_version(surface(), id), before);
    }

    #[test]
    fn a_non_contiguous_fill_over_many_non_matching_tiles_is_refused() {
        let (_dir, mut s) = store();
        // Ten held tiles, none matching the (transparent) seed.
        for tx in 0..10_u32 {
            put(&mut s, tx * TILE, 0, grey(1.0));
            fill_rect(&mut s, tx * TILE, 0, tx * TILE + 256, 1, grey(1.0));
        }
        let mut snapshot = StrokeSnapshot::new(surface());
        let result = flood_fill(
            &mut s,
            surface(),
            (10 * TILE, 256),
            &FloodSpec {
                seed: (5, 200),
                tolerance: 0,
                contiguous: false,
                max_tiles: MAX_FILL_TILES,
                max_examined: 4,
            },
            RED,
            FillMode::Normal,
            Some(&mut snapshot),
        );
        assert!(
            matches!(result, Err(FillError::TooLarge { max: 4 })),
            "{result:?}"
        );
        assert!(snapshot.is_empty(), "nothing was written");
        // The same layer with the cap raised is fine.
        let ok = flood_fill(
            &mut s,
            surface(),
            (10 * TILE, 256),
            &FloodSpec {
                seed: (5, 200),
                tolerance: 0,
                contiguous: false,
                max_tiles: MAX_FILL_TILES,
                max_examined: 10,
            },
            RED,
            FillMode::Normal,
            None,
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn coordinates_at_the_u32_limit_do_not_overflow() {
        let (_dir, mut s) = store();
        let far = 299_997;
        let spec_far = FloodSpec {
            seed: (far, far),
            tolerance: 0,
            contiguous: true,
            max_tiles: 4,
            max_examined: 4,
        };
        let r = flood_fill(
            &mut s,
            surface(),
            (300_000, 300_000),
            &spec_far,
            RED,
            FillMode::Normal,
            None,
        );
        assert!(matches!(r, Err(FillError::TooLarge { .. })), "{r:?}");
        let mut grad = linear([0.0; 3], [1.0; 3]);
        grad.max_tiles = 4;
        let r = gradient_fill(
            &mut s,
            surface(),
            (u32::MAX - 5, 0, u32::MAX, 3),
            &grad,
            FillMode::Normal,
            None,
        );
        assert!(r.is_ok(), "{r:?}");
        let mut seen = Region::new(4);
        assert!(seen.set_span(0, u32::MAX - 3, u32::MAX).is_ok());
        assert!(seen.contains(u32::MAX, 0));
    }

    #[test]
    fn a_fill_is_undone_exactly_by_its_snapshot() {
        let (_dir, mut s) = store();
        fill_rect(&mut s, 250, 250, 262, 262, grey(0.5));
        let before: Vec<[f32; 4]> = (250..262).map(|x| get(&mut s, x, 255)).collect();
        let Ok((painted, snapshot)) = bucket(
            &mut s,
            (300, 300),
            &spec((255, 255), 0, true),
            FillMode::Normal,
        ) else {
            unreachable!()
        };
        assert!(painted > 0);
        assert_eq!(snapshot.captured().count(), painted);
        let Ok(redo) = snapshot.apply(&mut s) else {
            unreachable!("apply")
        };
        let after: Vec<[f32; 4]> = (250..262).map(|x| get(&mut s, x, 255)).collect();
        assert_eq!(before, after);
        let Ok(_) = redo.apply(&mut s) else {
            unreachable!()
        };
        assert!(is_red(get(&mut s, 255, 255)));
    }

    // -- gradient ---------------------------------------------------------

    fn linear(from: [f32; 3], to: [f32; 3]) -> GradientSpec {
        GradientSpec {
            kind: GradientKind::Linear,
            start: (0.0, 0.0),
            end: (10.0, 0.0),
            from,
            to,
            opacity: 1.0,
            max_tiles: MAX_FILL_TILES,
        }
    }

    fn gradient(
        s: &mut TileStore,
        area: (u32, u32, u32, u32),
        spec: &GradientSpec,
        mode: FillMode,
    ) -> Result<(usize, StrokeSnapshot), FillError> {
        let mut snapshot = StrokeSnapshot::new(surface());
        let outcome = gradient_fill(s, surface(), area, spec, mode, Some(&mut snapshot))?;
        Ok((outcome.painted().len(), snapshot))
    }

    #[test]
    fn linear_endpoints_are_exactly_the_two_colours_and_the_middle_is_the_mean() {
        let (_dir, mut s) = store();
        let spec = linear([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let Ok(_) = gradient(&mut s, (0, 0, 20, 1), &spec, FillMode::Normal) else {
            unreachable!()
        };
        // Behind the start and beyond the end are clamped.
        assert_eq!(get(&mut s, 0, 0)[3], 1.0);
        let end = get(&mut s, 15, 0);
        assert_eq!(end, [1.0; 4], "past the end: exactly the second colour");
        let middle = get(&mut s, 4, 0);
        // pixel 4 centre 4.5 of 10 -> t = 0.45
        assert!((middle[0] - 0.45).abs() < 1e-3, "{middle:?}");
        let near_start = get(&mut s, 0, 0);
        assert!((near_start[0] - 0.05).abs() < 1e-3, "{near_start:?}");
    }

    #[test]
    fn a_drag_starting_inside_the_layer_clamps_to_the_first_colour_behind_it() {
        let (_dir, mut s) = store();
        let mut spec = linear([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        spec.start = (10.0, 0.0);
        spec.end = (20.0, 0.0);
        let Ok(_) = gradient(&mut s, (0, 0, 30, 1), &spec, FillMode::Normal) else {
            unreachable!()
        };
        assert_eq!(get(&mut s, 2, 0), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(get(&mut s, 25, 0), [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn swapping_the_colours_mirrors_the_gradient() {
        let (_dir, mut a) = store();
        let (_dir2, mut b) = store();
        let forward = linear([0.0, 0.2, 1.0], [1.0, 0.6, 0.0]);
        let reverse = linear([1.0, 0.6, 0.0], [0.0, 0.2, 1.0]);
        let Ok(_) = gradient(&mut a, (0, 0, 10, 1), &forward, FillMode::Normal) else {
            unreachable!()
        };
        let Ok(_) = gradient(&mut b, (0, 0, 10, 1), &reverse, FillMode::Normal) else {
            unreachable!()
        };
        let pa = get(&mut a, 2, 0);
        let pb = get(&mut b, 2, 0);
        assert!((pa[0] - (1.0 - pb[0])).abs() < 1e-3);
        assert_ne!(pa, pb);
    }

    #[test]
    fn a_radial_gradient_depends_only_on_the_distance_from_the_start() {
        let (_dir, mut s) = store();
        let spec = GradientSpec {
            kind: GradientKind::Radial,
            start: (20.0, 20.0),
            end: (30.0, 20.0),
            from: [0.0; 3],
            to: [1.0; 3],
            opacity: 1.0,
            max_tiles: MAX_FILL_TILES,
        };
        let Ok(_) = gradient(&mut s, (0, 0, 40, 40), &spec, FillMode::Normal) else {
            unreachable!()
        };
        // Same distance (5 px) to the right and straight down.
        let right = get(&mut s, 25, 20);
        let down = get(&mut s, 20, 25);
        assert_eq!(right, down);
        assert!((right[0] - 0.5).abs() < 0.06, "{right:?}");
        assert!(get(&mut s, 20, 20)[0] < 0.1);
        assert_eq!(
            get(&mut s, 39, 39),
            [1.0; 4],
            "beyond the radius: the second colour"
        );
        // A linear gradient with the same drag differs at the down pixel.
        let (_dir, mut l) = store();
        let mut lin = spec;
        lin.kind = GradientKind::Linear;
        let Ok(_) = gradient(&mut l, (0, 0, 40, 40), &lin, FillMode::Normal) else {
            unreachable!()
        };
        assert_ne!(get(&mut l, 20, 25), get(&mut s, 20, 25));
    }

    #[test]
    fn opacity_blends_over_the_existing_pixels() {
        let (_dir, mut s) = store();
        fill_rect(
            &mut s,
            0,
            0,
            4,
            1,
            [f16::ONE, f16::ZERO, f16::ZERO, f16::ONE],
        );
        let mut spec = linear([0.0, 0.0, 1.0], [0.0, 0.0, 1.0]);
        spec.opacity = 0.5;
        let Ok(_) = gradient(&mut s, (0, 0, 4, 1), &spec, FillMode::Normal) else {
            unreachable!()
        };
        assert_eq!(get(&mut s, 1, 0), [0.5, 0.0, 0.5, 1.0]);
        // Over transparent pixels the result is the colour at 50 % alpha.
        let mut spec = linear([0.0, 0.0, 1.0], [0.0, 0.0, 1.0]);
        spec.opacity = 0.5;
        let Ok(_) = gradient(&mut s, (0, 1, 4, 2), &spec, FillMode::Normal) else {
            unreachable!()
        };
        assert_eq!(get(&mut s, 1, 1), [0.0, 0.0, 0.5, 0.5]);
        // Opacity zero changes nothing.
        let mut spec = linear([0.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
        spec.opacity = 0.0;
        let Ok((painted, snapshot)) = gradient(&mut s, (0, 2, 4, 3), &spec, FillMode::Normal)
        else {
            unreachable!()
        };
        assert_eq!(painted, 0);
        assert!(snapshot.is_empty());
    }

    #[test]
    fn a_zero_length_drag_is_a_no_op() {
        let (_dir, mut s) = store();
        let mut spec = linear([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        spec.end = spec.start;
        let Ok((painted, snapshot)) = gradient(&mut s, (0, 0, 8, 8), &spec, FillMode::Normal)
        else {
            unreachable!()
        };
        assert_eq!(painted, 0);
        assert!(snapshot.is_empty());
        spec.end = (f32::NAN, 0.0);
        assert!(spec.is_degenerate());
    }

    #[test]
    fn a_transparency_locked_gradient_keeps_alpha_and_skips_transparent_pixels() {
        let (_dir, mut s) = store();
        let half = f16::from_f32(0.5);
        put(&mut s, 0, 0, [half, half, half, half]);
        let spec = linear([0.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
        let Ok(_) = gradient(&mut s, (0, 0, 2, 1), &spec, FillMode::PreserveAlpha) else {
            unreachable!()
        };
        assert_eq!(get(&mut s, 0, 0), [0.0, 0.5, 0.0, 0.5]);
        assert_eq!(get(&mut s, 1, 0), [0.0; 4]);
    }

    #[test]
    fn a_gradient_over_too_many_tiles_is_refused_before_any_write() {
        let (_dir, mut s) = store();
        let mut spec = linear([0.0; 3], [1.0; 3]);
        spec.max_tiles = 3;
        let mut snapshot = StrokeSnapshot::new(surface());
        let result = gradient_fill(
            &mut s,
            surface(),
            (0, 0, 1024, 256),
            &spec,
            FillMode::Normal,
            Some(&mut snapshot),
        );
        assert!(matches!(result, Err(FillError::TooLarge { max: 3 })));
        assert!(snapshot.is_empty());
        assert_eq!(s.resident_len(), 0);
        let ok = gradient_fill(
            &mut s,
            surface(),
            (0, 0, 768, 256),
            &spec,
            FillMode::Normal,
            None,
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn a_gradient_across_tiles_is_seamless_and_undone_exactly() {
        let (_dir, mut s) = store();
        let mut spec = linear([0.0; 3], [1.0; 3]);
        spec.start = (200.0, 0.0);
        spec.end = (320.0, 0.0);
        let Ok((painted, snapshot)) = gradient(&mut s, (0, 0, 512, 4), &spec, FillMode::Normal)
        else {
            unreachable!()
        };
        assert_eq!(painted, 2);
        // Continuity across the x = 256 tile edge.
        let (a, b) = (get(&mut s, 255, 0)[0], get(&mut s, 256, 0)[0]);
        assert!(b > a && b - a < 0.02, "{a} {b}");
        let Ok(_) = snapshot.apply(&mut s) else {
            unreachable!()
        };
        assert_eq!(get(&mut s, 255, 0), [0.0; 4]);
        assert_eq!(get(&mut s, 300, 3), [0.0; 4]);
    }

    #[test]
    fn the_gradient_parameter_is_clamped_and_total() {
        let spec = linear([0.0; 3], [1.0; 3]);
        assert_eq!(spec.parameter(-5.0, 0.0), Some(0.0));
        assert_eq!(spec.parameter(50.0, 0.0), Some(1.0));
        assert_eq!(
            spec.parameter(5.0, 100.0),
            Some(0.5),
            "linear ignores the perpendicular"
        );
        assert_eq!(spec.colour_at(0.0), [0.0; 3]);
        assert_eq!(spec.colour_at(1.0), [1.0; 3]);
    }
}
