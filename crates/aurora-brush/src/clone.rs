//! The Clone Stamp's dab (0.221.0): copies the texels of the same layer at a
//! fixed integer offset into the Brush's soft footprint.
//!
//! **What a dab writes.** For every texel `p` of the Brush's footprint
//! (centre not snapped, falloff `a = dab_falloff(d, radius)`, exactly the
//! texels [`crate::stamp_dab`] visits) the source is the texel at `p +
//! offset` on the same surface, premultiplied `f16`. The destination moves
//! toward it by the falloff, per premultiplied channel (alpha included):
//! `new = old + (source - old) * a`, and exactly `source` at `a >= 1`. This
//! is **not** the Brush's max-alpha rule: that rule never changes a texel
//! whose alpha is already at or above the dab's (so a Brush over opaque
//! pixels paints nothing), which would make cloning onto an opaque layer a
//! no-op. A move toward the source converges (a stroke's overlapping dabs build up
//! on the source and never overshoot it; at falloff `1` it is an exact copy)
//! and copies the source's transparency along with its
//! colour: cloning an empty area onto pixels fades them out. That is a
//! choice, not a measured Photoshop rule.
//!
//! **Where the source is read from.** The layer *as it was when the stroke
//! began*, never the stroke's own output: the stroke's [`StrokeSnapshot`]
//! already holds the pre-stroke copy of every tile the stroke has written
//! (a raw copy, taken just before the tile's first write), and every other
//! tile is untouched, so its live content is its stroke-start content. A
//! tile the stroke has changed is read from the snapshot, all others from
//! the store, and nothing is copied beyond the bounded patch below. The
//! patch is gathered before the dab writes a single texel, so a dab whose
//! source overlaps its own footprint does not clone itself either.
//!
//! **Bounds.** The source patch covers the dab's bounding box (never more
//! than [`MAX_CLONE_PATCH_TEXELS`]: a dab over that is refused whole, like
//! one over [`crate::stamp`]'s tile cap). A source tile that is absent, or
//! a source texel left of or above the surface's origin, is transparent. A
//! source tile that cannot be read refuses the whole dab (reported in
//! [`DabOutcome::failed`]): writing transparency in its place would erase.
//!
//! The write loop is [`crate::stamp_dab`]'s: capture-before-first-write,
//! selection coverage blended per texel, no phantom tile (a tile the store
//! does not hold is created only when the dab will really write into it),
//! `stores_the_same_bits` so an unchanged texel is neither captured nor
//! reported painted. A **transparency lock** (`preserve_alpha`) keeps each
//! texel's alpha bit for bit and moves only its colour toward the source's
//! straight colour (at the destination's alpha) by the falloff; a source
//! texel with no alpha, or a destination texel with none, is left alone.

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::StrokeSnapshot;
use crate::coverage::{Coverage, blend_texel, covered};
use crate::stamp::{
    ALPHA, DabOutcome, Texel, is_paintable, malformed_tile, reserved_tiles, stores_the_same_bits,
    texel_alpha, texel_index, tile_coverage, tile_range, usable_snapshot,
};

/// The most texels one dab's source patch may hold (a 1024 x 1024 box, a
/// radius of about 512): an engineering default that bounds the patch to
/// 8 MiB of `f16`. A larger dab is refused whole and logged.
pub const MAX_CLONE_PATCH_TEXELS: u64 = 1 << 20;

/// The source patch of one dab: `width * height` premultiplied texels (four
/// `f16` each, row-major) whose top-left destination texel is `(x0, y0)`.
struct Patch {
    x0: u32,
    y0: u32,
    width: u32,
    texels: Vec<f16>,
}

impl Patch {
    /// The source texel for destination texel `(x, y)`, transparent when
    /// outside the patch.
    fn texel(&self, x: u32, y: u32) -> Texel {
        let zero = f16::from_f32(0.0);
        let (Some(px), Some(py)) = (x.checked_sub(self.x0), y.checked_sub(self.y0)) else {
            return [zero; CHANNELS];
        };
        if px >= self.width {
            return [zero; CHANNELS];
        }
        let index = (py as usize * self.width as usize + px as usize) * CHANNELS;
        match self.texels.get(index..index + CHANNELS) {
            Some([red, green, blue, alpha]) => [*red, *green, *blue, *alpha],
            _ => [zero; CHANNELS],
        }
    }
}

/// Gathers the source patch for the destination box `x0..=x1` x `y0..=y1`
/// shifted by `offset`, from the stroke-start content of `surface`.
/// `Err` is the tile that could not be read.
#[allow(clippy::too_many_arguments)]
fn gather_patch(
    store: &mut TileStore,
    surface: SurfaceId,
    snapshot: Option<&StrokeSnapshot>,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    offset: (i64, i64),
) -> Result<Patch, (TileId, aurora_tile::TileError)> {
    let width = x1 - x0 + 1;
    let height = y1 - y0 + 1;
    let mut texels = vec![f16::from_f32(0.0); width as usize * height as usize * CHANNELS];
    // The source box in surface space; the surface has no negative texels.
    let sx0 = (i64::from(x0) + offset.0).max(0);
    let sy0 = (i64::from(y0) + offset.1).max(0);
    let sx1 = i64::from(x1) + offset.0;
    let sy1 = i64::from(y1) + offset.1;
    if sx1 < 0 || sy1 < 0 || sx0 > sx1 || sy0 > sy1 {
        return Ok(Patch {
            x0,
            y0,
            width,
            texels,
        });
    }
    let tile = i64::from(TILE);
    let max_tile = i64::from(u32::MAX / TILE);
    let (tx0, tx1) = ((sx0 / tile).min(max_tile), (sx1 / tile).min(max_tile));
    let (ty0, ty1) = ((sy0 / tile).min(max_tile), (sy1 / tile).min(max_tile));
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let id = TileId {
                x: tx as u32,
                y: ty as u32,
            };
            let source: Vec<f16>;
            let content: &[f16] = if let Some(raw) = snapshot.and_then(|s| s.raw_content(id)) {
                raw
            } else if store.contains_tile(surface, id) {
                match store.get(surface, id) {
                    Ok(tile) => {
                        source = tile.texels().to_vec();
                        &source
                    }
                    Err(err) => return Err((id, err)),
                }
            } else {
                continue;
            };
            // The overlap of this tile with the source box, in surface space.
            let ox0 = sx0.max(tx * tile);
            let oy0 = sy0.max(ty * tile);
            let ox1 = sx1.min(tx * tile + tile - 1);
            let oy1 = sy1.min(ty * tile + tile - 1);
            for sy in oy0..=oy1 {
                for sx in ox0..=ox1 {
                    // Destination texel and patch slot, then the tile slot.
                    let dx = sx - offset.0 - i64::from(x0);
                    let dy = sy - offset.1 - i64::from(y0);
                    let (Ok(dx), Ok(dy)) = (usize::try_from(dx), usize::try_from(dy)) else {
                        continue;
                    };
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let (lx, ly) = ((sx - tx * tile) as u32, (sy - ty * tile) as u32);
                    let from = texel_index(lx, ly);
                    let to = (dy * width as usize + dx) * CHANNELS;
                    if let (Some(src), Some(dst)) = (
                        content.get(from..from + CHANNELS),
                        texels.get_mut(to..to + CHANNELS),
                    ) {
                        dst.copy_from_slice(src);
                    }
                }
            }
        }
    }
    Ok(Patch {
        x0,
        y0,
        width,
        texels,
    })
}

/// The texel a clone dab writes over `current`, or `None` to leave it:
/// `source` weighted by the footprint's falloff `a` (see the module docs).
fn clone_texel(current: &[f16], source: Texel, a: f32, preserve_alpha: bool) -> Option<Texel> {
    let old = |c: usize| current.get(c).map_or(0.0, |v| v.to_f32());
    if preserve_alpha {
        let dst_alpha = old(ALPHA);
        let src_alpha = source.get(ALPHA).map_or(0.0, |v| v.to_f32());
        if dst_alpha <= 0.0 || src_alpha <= 0.0 {
            return None;
        }
        let toward = |c: usize| {
            let straight = source.get(c).map_or(0.0, |v| v.to_f32()) / src_alpha;
            let target = straight * dst_alpha;
            f16::from_f32(old(c) + (target - old(c)) * a)
        };
        // Alpha kept bit for bit.
        let kept = current.get(ALPHA).copied()?;
        return Some([toward(0), toward(1), toward(2), kept]);
    }
    if a >= 1.0 {
        return Some(source);
    }
    let mix = |c: usize| {
        let s = source.get(c).map_or(0.0, |v| v.to_f32());
        f16::from_f32(old(c) + (s - old(c)) * a)
    };
    Some([mix(0), mix(1), mix(2), mix(3)])
}

/// Whether any texel of `tile_box` would be written into a tile the store
/// does not hold: some texel in the footprint, covered, with a source that
/// has alpha. Only consulted for such a tile, so a clone of empty space
/// creates no tile.
#[allow(clippy::cast_precision_loss)]
fn reaches_blank_tile(
    center: (f32, f32),
    radius: f32,
    patch: &Patch,
    origin: (u32, u32),
    tile_box: (u32, u32, u32, u32),
    coverage: Option<&dyn Coverage>,
) -> bool {
    let (lx0, ly0, lx1, ly1) = tile_box;
    for ly in ly0..=ly1 {
        for lx in lx0..=lx1 {
            let (x, y) = (origin.0 + lx, origin.1 + ly);
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            if texel_alpha(px, py, center, radius, false) <= 0.0 {
                continue;
            }
            if patch.texel(x, y)[ALPHA].to_f32() <= 0.0 {
                continue;
            }
            if coverage.is_none_or(|cov| covered(cov, i64::from(x), i64::from(y)) > 0.0) {
                return true;
            }
        }
    }
    false
}

/// One Clone Stamp dab (see the module docs): the Brush's soft footprint at
/// `center` (layer-local texels) filled from the same surface `offset`
/// texels away, as it was when the stroke began. `snapshot` is the stroke's
/// own undo snapshot: it captures what the dab writes and is where the
/// source of an already-written tile is read from. `coverage` restricts the
/// dab to a selection exactly as [`crate::stamp_dab_covered`]'s does;
/// `preserve_alpha` is the transparency lock.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
pub fn clone_dab_covered(
    store: &mut TileStore,
    surface: SurfaceId,
    center: (f32, f32),
    radius: f32,
    offset: (i64, i64),
    preserve_alpha: bool,
    snapshot: Option<&mut StrokeSnapshot>,
    coverage: Option<&dyn Coverage>,
) -> DabOutcome {
    let mut outcome = DabOutcome::default();
    if !is_paintable(center, radius) {
        return outcome;
    }
    let mut snapshot = usable_snapshot(snapshot, surface);
    let (cx, cy) = center;
    let min_x = (cx - radius).floor().max(0.0) as u32;
    let min_y = (cy - radius).floor().max(0.0) as u32;
    let max_x = (cx + radius).ceil().max(0.0) as u32;
    let max_y = (cy + radius).ceil().max(0.0) as u32;
    if max_x < min_x || max_y < min_y {
        return outcome;
    }
    let texels = u64::from(max_x - min_x + 1) * u64::from(max_y - min_y + 1);
    if texels > MAX_CLONE_PATCH_TEXELS {
        tracing::warn!(
            texels,
            max = MAX_CLONE_PATCH_TEXELS,
            "refusing a clone dab whose source patch is over the cap; it paints nothing"
        );
        return outcome;
    }
    // The whole source patch is read before anything is written.
    let patch = match gather_patch(
        store,
        surface,
        snapshot.as_deref(),
        min_x,
        min_y,
        max_x,
        max_y,
        offset,
    ) {
        Ok(patch) => patch,
        Err((id, err)) => {
            outcome.fail(id, err);
            return outcome;
        }
    };
    let (t0, t1) = tile_range(center, radius);
    outcome.painted = Vec::with_capacity(reserved_tiles(t0, t1));

    for ty in t0.y..=t1.y {
        for tx in t0.x..=t1.x {
            let id = TileId { x: tx, y: ty };
            let origin_x = tx * TILE;
            let origin_y = ty * TILE;
            let lx0 = min_x.saturating_sub(origin_x).min(TILE - 1);
            let ly0 = min_y.saturating_sub(origin_y).min(TILE - 1);
            let lx1 = max_x.saturating_sub(origin_x).min(TILE - 1);
            let ly1 = max_y.saturating_sub(origin_y).min(TILE - 1);
            let Some(tile_cov) =
                tile_coverage(coverage, (origin_x, origin_y), (lx0, ly0, lx1, ly1))
            else {
                continue;
            };
            if !store.contains_tile(surface, id)
                && (preserve_alpha
                    || !reaches_blank_tile(
                        center,
                        radius,
                        &patch,
                        (origin_x, origin_y),
                        (lx0, ly0, lx1, ly1),
                        tile_cov,
                    ))
            {
                continue;
            }
            let tile = match store.get_mut(surface, id) {
                Ok(tile) => tile,
                Err(err) => {
                    outcome.fail(id, err);
                    continue;
                }
            };
            let mut touched_this_tile = false;
            let mut capture_failed = false;
            let tile_texels = tile.texels_mut();

            'texels: for ly in ly0..=ly1 {
                for lx in lx0..=lx1 {
                    let (x, y) = (origin_x + lx, origin_y + ly);
                    #[allow(clippy::cast_precision_loss)]
                    let px = x as f32 + 0.5;
                    #[allow(clippy::cast_precision_loss)]
                    let py = y as f32 + 0.5;
                    let a = texel_alpha(px, py, center, radius, false);
                    if a <= 0.0 {
                        continue;
                    }
                    let cover = match tile_cov {
                        Some(cov) => {
                            let c = covered(cov, i64::from(x), i64::from(y));
                            if c <= 0.0 {
                                continue;
                            }
                            c
                        }
                        None => 1.0,
                    };
                    let index = texel_index(lx, ly);
                    let Some(current) = tile_texels.get(index..index + CHANNELS) else {
                        continue;
                    };
                    let Some(fresh) = clone_texel(current, patch.texel(x, y), a, preserve_alpha)
                    else {
                        continue;
                    };
                    let fresh = if cover < 1.0 {
                        blend_texel(current, fresh, cover)
                    } else {
                        fresh
                    };
                    if stores_the_same_bits(current, fresh) {
                        continue;
                    }
                    if !touched_this_tile {
                        if let Some(stroke) = snapshot.as_mut()
                            && !stroke.record_content(id, tile_texels)
                        {
                            capture_failed = true;
                            break 'texels;
                        }
                        touched_this_tile = true;
                    }
                    for (channel, &value) in fresh.iter().enumerate() {
                        if let Some(sample) = tile_texels.get_mut(index + channel) {
                            *sample = value;
                        }
                    }
                }
            }
            if capture_failed {
                outcome.fail(id, malformed_tile(surface, id, tile_texels.len()));
                continue;
            }
            if touched_this_tile {
                outcome.painted.push(id);
                tile.mark_dirty(Rect {
                    x: i64::from(lx0),
                    y: i64::from(ly0),
                    width: lx1 - lx0 + 1,
                    height: ly1 - ly0 + 1,
                });
            }
        }
    }
    outcome
}
