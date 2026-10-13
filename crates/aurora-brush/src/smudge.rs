//! The Smudge tool's dab (0.222.0): drags the layer's own pixels along the
//! stroke, in the Brush's soft footprint (finger painting off).
//!
//! **The carried patch.** A stroke owns a [`SmudgeCarry`]: premultiplied
//! RGBA `f32` texels for the square of side `2h + 1` around the dab's
//! *anchor* `(floor(cx), floor(cy))`, with `h = ceil(radius) + 1` (so it
//! holds the whole footprint wherever inside a texel the centre falls).
//! A texel is addressed by its offset from the anchor, which is what makes
//! the patch "travel with the finger": the same offset is read at the next
//! dab, whose anchor has moved on. It is empty until the first dab of a
//! stroke and the app creates a fresh one at every press, so nothing is
//! carried between strokes, tools or documents.
//!
//! **What a dab does**, with `s` the strength in `0..=1` and `a =
//! dab_falloff(d, radius)` the Brush's falloff at distance `d` from the
//! centre (exactly the texels [`crate::stamp_dab`] visits):
//!
//! 1. *Read.* Every texel of the patch square is read from the layer as it
//!    is now (premultiplied, an absent tile or a texel left of or above the
//!    origin is transparent, a non-finite sample counts as `0`). This is
//!    `under`. The layer is the stroke's **own current state**: the previous
//!    dabs of the stroke are the point, unlike the Clone Stamp.
//! 2. *Pick up.* If the carry is empty (the first dab of the stroke), or was
//!    made for a different radius, it becomes `under` and the dab changes
//!    nothing: the finger lands.
//! 3. *Deposit.* Each footprint texel moves toward the carried texel at its
//!    offset by `k = s * a`, per premultiplied channel, alpha included:
//!    `new = old + (carried - old) * k` (exactly the carried texel at `k >=
//!    1`). Coverage (a selection) then blends `old` toward `new` by the
//!    coverage, as every other tool does.
//! 4. *Mix.* Afterwards, for every texel of the patch, `carried = carried *
//!    s + under * (1 - s)`: `under` being the values read in step 1 (before
//!    the dab wrote), whether or not the texel was inside the footprint,
//!    covered by the selection or written. At `s = 1` the finger keeps its
//!    paint unchanged for the whole stroke and picks up nothing; at `s = 0`
//!    it carries only what it is over and deposits nothing; in between the
//!    paint it carries is progressively replaced by what it passes over. At
//!    a footprint texel with `a = 1` the new carried texel equals the texel
//!    just written.
//!
//! All of this is a **choice, not compared with Photoshop's Smudge**.
//!
//! A **transparency lock** (`preserve_alpha`) keeps each destination
//! texel's alpha bit for bit and moves only its colour toward the carried
//! straight colour at the destination's alpha; a texel with no alpha, or a
//! carried texel with none, is left alone. The carry itself is unaffected.
//!
//! **Bounds and failure.** The patch square never holds more than
//! [`MAX_SMUDGE_PATCH_TEXELS`]: a dab over that, a centre beyond `2^31`
//! texels, or a strength that is not a number is refused whole and touches
//! nothing, the carry included. An unreadable tile refuses the whole dab
//! before anything is written (reported in [`DabOutcome::failed`]), the
//! carry untouched.
//!
//! The write loop is [`crate::stamp_dab`]'s: capture-before-first-write,
//! selection coverage per texel, no phantom tile (a tile the store does not
//! hold is created only when the dab will really write into it),
//! `stores_the_same_bits` so an unchanged texel is neither captured nor
//! reported painted.

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::StrokeSnapshot;
use crate::coverage::{Coverage, blend_texel, covered};
use crate::stamp::{
    ALPHA, DabOutcome, Texel, is_paintable, malformed_tile, reserved_tiles, stores_the_same_bits,
    texel_alpha, texel_index, tile_coverage, tile_range, usable_snapshot,
};

/// The most texels the carried patch square may hold (1024 x 1024, a radius
/// of about 510): an engineering default that bounds it to 16 MiB of `f32`
/// (plus as much again while a dab runs). A larger dab is refused whole.
pub const MAX_SMUDGE_PATCH_TEXELS: u64 = 1 << 20;

/// The largest centre coordinate magnitude a smudge dab accepts (`2^31`
/// texels), so the patch arithmetic stays far from overflow.
const MAX_CENTER: f32 = 2_147_483_648.0;

/// The paint a Smudge stroke carries (see the module docs). `Default` is
/// the empty carry a press starts with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmudgeCarry {
    /// `ceil(radius) + 1` of the dab that made it; `0` while empty.
    half: i64,
    /// `(2 * half + 1)^2` premultiplied RGBA texels, row-major.
    texels: Vec<f32>,
}

impl SmudgeCarry {
    /// Whether nothing is carried yet (before the stroke's first dab).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.texels.is_empty()
    }

    /// Forgets everything carried.
    pub fn clear(&mut self) {
        self.half = 0;
        self.texels.clear();
    }

    /// The carried premultiplied texel at offset `(dx, dy)` from the dab's
    /// anchor `(floor(cx), floor(cy))`, `None` outside the patch or while
    /// empty.
    #[must_use]
    pub fn texel_at(&self, dx: i64, dy: i64) -> Option<[f32; CHANNELS]> {
        let side = 2 * self.half + 1;
        let (px, py) = (dx + self.half, dy + self.half);
        if self.texels.is_empty() || px < 0 || py < 0 || px >= side || py >= side {
            return None;
        }
        let index = usize::try_from(py * side + px).ok()? * CHANNELS;
        match self.texels.get(index..index + CHANNELS) {
            Some([r, g, b, a]) => Some([*r, *g, *b, *a]),
            _ => None,
        }
    }
}

/// The patch half-extent a dab of `radius` uses.
#[allow(clippy::cast_possible_truncation)]
fn half_extent(radius: f32) -> i64 {
    radius.ceil() as i64 + 1
}

/// Reads the layer under the patch square around `anchor` as premultiplied
/// `f32` (see the module docs). `Err` is the tile that could not be read.
fn gather_under(
    store: &mut TileStore,
    surface: SurfaceId,
    anchor: (i64, i64),
    half: i64,
) -> Result<Vec<f32>, (TileId, aurora_tile::TileError)> {
    let side = 2 * half + 1;
    #[allow(clippy::cast_sign_loss)]
    let mut under = vec![0.0_f32; (side * side) as usize * CHANNELS];
    let (x0, y0) = (anchor.0 - half, anchor.1 - half);
    let (x1, y1) = (anchor.0 + half, anchor.1 + half);
    if x1 < 0 || y1 < 0 {
        return Ok(under);
    }
    let tile = i64::from(TILE);
    let max_tile = i64::from(u32::MAX / TILE);
    let (tx0, tx1) = ((x0.max(0) / tile).min(max_tile), (x1 / tile).min(max_tile));
    let (ty0, ty1) = ((y0.max(0) / tile).min(max_tile), (y1 / tile).min(max_tile));
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
            let tile_texels = match store.get(surface, id) {
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
                    let to = ((y - y0) * side + (x - x0)) as usize * CHANNELS;
                    if let (Some(src), Some(dst)) = (
                        tile_texels.get(from..from + CHANNELS),
                        under.get_mut(to..to + CHANNELS),
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
    Ok(under)
}

/// The texel a smudge dab writes over `current`, or `None` to leave it:
/// `carried` weighted by `k = s * a` (see the module docs).
fn smudge_texel(
    current: &[f16],
    carried: [f32; CHANNELS],
    k: f32,
    preserve_alpha: bool,
) -> Option<Texel> {
    let old = |c: usize| current.get(c).map_or(0.0, |v| v.to_f32());
    let carry_of = |c: usize| carried.get(c).copied().unwrap_or(0.0);
    if preserve_alpha {
        let dst_alpha = old(ALPHA);
        let src_alpha = carry_of(ALPHA);
        if dst_alpha <= 0.0 || src_alpha <= 0.0 {
            return None;
        }
        let toward = |c: usize| {
            let target = carry_of(c) / src_alpha * dst_alpha;
            f16::from_f32(old(c) + (target - old(c)) * k)
        };
        let kept = current.get(ALPHA).copied()?;
        return Some([toward(0), toward(1), toward(2), kept]);
    }
    if k >= 1.0 {
        return Some(carried.map(f16::from_f32));
    }
    let mix = |c: usize| f16::from_f32(old(c) + (carry_of(c) - old(c)) * k);
    Some([mix(0), mix(1), mix(2), mix(3)])
}

/// The carried texel for the patch slot of destination texel `(x, y)`.
fn carried_at(carry: &SmudgeCarry, anchor: (i64, i64), x: u32, y: u32) -> Option<[f32; CHANNELS]> {
    carry.texel_at(i64::from(x) - anchor.0, i64::from(y) - anchor.1)
}

/// Whether any texel of `tile_box` would change in a tile the store does not
/// hold (every sample `0`): some covered footprint texel whose smudged
/// result is not transparent black. Only consulted for such a tile, so
/// smudging empty space with an empty finger creates no tile.
#[allow(clippy::too_many_arguments, clippy::cast_precision_loss)]
fn reaches_blank_tile(
    center: (f32, f32),
    radius: f32,
    strength: f32,
    preserve_alpha: bool,
    carry: &SmudgeCarry,
    anchor: (i64, i64),
    origin: (u32, u32),
    tile_box: (u32, u32, u32, u32),
    coverage: Option<&dyn Coverage>,
) -> bool {
    // A transparency lock keeps every alpha: a blank tile has none to keep.
    if preserve_alpha {
        return false;
    }
    let zero = [f16::from_f32(0.0); CHANNELS];
    let (lx0, ly0, lx1, ly1) = tile_box;
    for ly in ly0..=ly1 {
        for lx in lx0..=lx1 {
            let (x, y) = (origin.0 + lx, origin.1 + ly);
            let a = texel_alpha(x as f32 + 0.5, y as f32 + 0.5, center, radius, false);
            if a <= 0.0 {
                continue;
            }
            let Some(carried) = carried_at(carry, anchor, x, y) else {
                continue;
            };
            let cover = coverage.map_or(1.0, |cov| covered(cov, i64::from(x), i64::from(y)));
            if cover <= 0.0 {
                continue;
            }
            let Some(fresh) = smudge_texel(&zero, carried, strength * a, false) else {
                continue;
            };
            let fresh = if cover < 1.0 {
                blend_texel(&zero, fresh, cover)
            } else {
                fresh
            };
            if !stores_the_same_bits(&zero, fresh) {
                return true;
            }
        }
    }
    false
}

/// One Smudge dab (see the module docs): the Brush's soft footprint at
/// `center` (layer-local texels) pushed along by the paint `carry` holds,
/// which it then updates. `strength` is `0..=1` (clamped; NaN refuses the
/// dab). `snapshot` is the stroke's undo snapshot and captures what the dab
/// writes; `coverage` restricts the dab to a selection exactly as
/// [`crate::stamp_dab_covered`]'s does; `preserve_alpha` is the transparency
/// lock.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
pub fn smudge_dab_covered(
    store: &mut TileStore,
    surface: SurfaceId,
    center: (f32, f32),
    radius: f32,
    strength: f32,
    preserve_alpha: bool,
    carry: &mut SmudgeCarry,
    snapshot: Option<&mut StrokeSnapshot>,
    coverage: Option<&dyn Coverage>,
) -> DabOutcome {
    let mut outcome = DabOutcome::default();
    if strength.is_nan() || !is_paintable(center, radius) {
        return outcome;
    }
    if center.0.abs() > MAX_CENTER || center.1.abs() > MAX_CENTER {
        tracing::warn!(?center, "refusing a smudge dab with an absurd centre");
        return outcome;
    }
    let half = half_extent(radius);
    let side = 2 * half + 1;
    let texels = (side as u64).saturating_mul(side as u64);
    if texels > MAX_SMUDGE_PATCH_TEXELS {
        tracing::warn!(
            texels,
            max = MAX_SMUDGE_PATCH_TEXELS,
            "refusing a smudge dab whose patch is over the cap; it paints nothing"
        );
        return outcome;
    }
    let strength = strength.clamp(0.0, 1.0);
    let anchor = (center.0.floor() as i64, center.1.floor() as i64);

    // Everything the dab needs is read before anything is written.
    let under = match gather_under(store, surface, anchor, half) {
        Ok(under) => under,
        Err((id, err)) => {
            outcome.fail(id, err);
            return outcome;
        }
    };
    if carry.is_empty() || carry.half != half {
        // The finger lands: pick up what is under it and change nothing.
        carry.half = half;
        carry.texels = under;
        return outcome;
    }

    let mut snapshot = usable_snapshot(snapshot, surface);
    let (cx, cy) = center;
    let min_x = (cx - radius).floor().max(0.0) as u32;
    let min_y = (cy - radius).floor().max(0.0) as u32;
    let max_x = (cx + radius).ceil().max(0.0) as u32;
    let max_y = (cy + radius).ceil().max(0.0) as u32;
    let (t0, t1) = tile_range(center, radius);
    outcome.painted = Vec::with_capacity(reserved_tiles(t0, t1));

    if max_x >= min_x && max_y >= min_y {
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
                    && !reaches_blank_tile(
                        center,
                        radius,
                        strength,
                        preserve_alpha,
                        carry,
                        anchor,
                        (origin_x, origin_y),
                        (lx0, ly0, lx1, ly1),
                        tile_cov,
                    )
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
                        let a = texel_alpha(x as f32 + 0.5, y as f32 + 0.5, center, radius, false);
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
                        let Some(carried) = carried_at(carry, anchor, x, y) else {
                            continue;
                        };
                        let index = texel_index(lx, ly);
                        let Some(current) = tile_texels.get(index..index + CHANNELS) else {
                            continue;
                        };
                        let Some(fresh) =
                            smudge_texel(current, carried, strength * a, preserve_alpha)
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
    }

    // Mix what the finger passed over into what it carries.
    for (carried, &below) in carry.texels.iter_mut().zip(&under) {
        *carried = *carried * strength + below * (1.0 - strength);
    }
    outcome
}
