//! The Dodge, Burn and Sponge tools' dab (0.224.0): Photoshop's toning
//! brushes on one engine, working on the layer's own pixels under the
//! Brush's soft footprint.
//!
//! **What a dab does**, with `e` the exposure (or flow) in `0..=1` and
//! `a = dab_falloff(d, radius)` the Brush's falloff at distance `d` from
//! the centre (exactly the texels [`crate::stamp_dab`] visits), `k = e * a`.
//! Every footprint texel with alpha `> 0` is treated on its own:
//!
//! 1. *Straight colour.* The stored texel is premultiplied; the working
//!    colour is `s = premultiplied / alpha` per channel, in the **encoded
//!    sRGB** the document stores (not linear light). A texel with no alpha
//!    is left alone (these tools never add paint to empty space), and a
//!    non-finite sample leaves the texel alone.
//! 2. *Luminance* `Y = 0.2126 R + 0.7152 G + 0.0722 B` (Rec.709 weights) of
//!    the straight colour.
//! 3. *Range weight* (Dodge and Burn only), with `Y` clamped to `0..=1`:
//!    Shadows `(1 - Y)^2`, Midtones `4 Y (1 - Y)`, Highlights `Y^2`. All
//!    three lie in `0..=1`; they are fixed curves, not Photoshop's.
//! 4. *The move*, per straight channel, `t = k * w`:
//!    * **Dodge** (lighten) `new = s + max(1 - s, 0) * t`, towards white;
//!    * **Burn** (darken) `new = s - max(s, 0) * t`, towards black;
//!    * **Desaturate** `new = s + (Y - s) * k`, towards the texel's grey;
//!    * **Saturate** `new = clamp(s + (s - Y) * k, 0, 1)`, away from the grey.
//!
//!    The `max(.., 0)` terms make a Dodge never darken and a Burn never
//!    lighten even for a stored channel outside `0..=1`; the in-gamut
//!    results stay in `0..=1` without a clamp. Saturate clamps each
//!    channel on its own, which can shift the hue a little when a channel
//!    hits the end of the range (a choice).
//! 5. The result is re-premultiplied **by the texel's own alpha**, which is
//!    never changed: the destination alpha is written back bit for bit.
//!    So a transparency lock changes nothing about these tools, and a
//!    grey texel is unchanged by either Sponge mode.
//!
//! Coverage (a selection) then blends the old texel toward the new by the
//! coverage, as every other tool does.
//!
//! All of this is **a choice, not compared with Photoshop's toning tools**:
//! Photoshop's range curves, its exposure scale, its `Protect Tones` and
//! its Sponge arithmetic are not modelled, and the maths runs on encoded
//! sRGB values, not on linear light.
//!
//! **Bounds and failure.** A dab whose footprint box would hold more than
//! [`MAX_TONE_DAB_TEXELS`], a centre beyond `2^31` texels, or an amount
//! that is not a number is refused whole and touches nothing. An amount of
//! `0` changes nothing. An unreadable tile refuses the whole dab before
//! anything is written (reported in [`DabOutcome::failed`]).
//!
//! The write loop is [`crate::stamp_dab`]'s: capture-before-first-write,
//! selection coverage per texel, **no phantom tile** (a tile the store
//! does not hold has only transparent texels, which these tools leave, so
//! none is ever created), `stores_the_same_bits` so an unchanged texel is
//! neither captured nor reported painted.

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::StrokeSnapshot;
use crate::coverage::{Coverage, blend_texel, covered};
use crate::stamp::{
    ALPHA, DabOutcome, Texel, is_paintable, malformed_tile, reserved_tiles, stores_the_same_bits,
    texel_alpha, texel_index, tile_coverage, tile_range, usable_snapshot,
};

/// The most texels the dab's footprint box (`2 * ceil(radius) + 3` on a
/// side) may hold: 1024 x 1024, an engineering default shared with the
/// Blur and Sharpen. A larger dab is refused whole.
pub const MAX_TONE_DAB_TEXELS: u64 = 1 << 20;

/// The largest centre coordinate magnitude a dab accepts (`2^31` texels).
const MAX_CENTER: f32 = 2_147_483_648.0;

/// Rec.709 luma weights of the straight sRGB-encoded colour.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Which tones a Dodge or Burn dab reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToneRange {
    /// Weighted `(1 - Y)^2`: dark texels move most.
    Shadows,
    /// Weighted `4 Y (1 - Y)`: mid-grey moves most (the default).
    #[default]
    Midtones,
    /// Weighted `Y^2`: light texels move most.
    Highlights,
}

impl ToneRange {
    /// Every range, in the order the options bar offers them.
    pub const ALL: [Self; 3] = [Self::Shadows, Self::Midtones, Self::Highlights];
}

/// The Sponge's two modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpongeMode {
    /// Towards the texel's grey (the default).
    #[default]
    Desaturate,
    /// Away from the texel's grey.
    Saturate,
}

impl SpongeMode {
    /// Both modes, in the order the options bar offers them.
    pub const ALL: [Self; 2] = [Self::Desaturate, Self::Saturate];

    /// The dab operation of this mode.
    #[must_use]
    pub const fn op(self) -> ToneOp {
        match self {
            Self::Desaturate => ToneOp::Desaturate,
            Self::Saturate => ToneOp::Saturate,
        }
    }
}

/// What a toning dab does to a texel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneOp {
    /// Lightens the tones of the range.
    Dodge(ToneRange),
    /// Darkens the tones of the range.
    Burn(ToneRange),
    /// The Sponge's Saturate mode: away from the texel's grey.
    Saturate,
    /// The Sponge's Desaturate mode: towards the texel's grey.
    Desaturate,
}

/// The range weight of straight luminance `y` (`0..=1` after a clamp; a
/// non-finite `y` weighs `0`), in `0..=1`: see the module docs.
#[must_use]
pub fn range_weight(range: ToneRange, y: f32) -> f32 {
    if !y.is_finite() {
        return 0.0;
    }
    let y = y.clamp(0.0, 1.0);
    match range {
        ToneRange::Shadows => (1.0 - y) * (1.0 - y),
        ToneRange::Midtones => 4.0 * y * (1.0 - y),
        ToneRange::Highlights => y * y,
    }
}

/// The Rec.709 luminance of a straight colour.
#[must_use]
pub fn luminance(rgb: [f32; 3]) -> f32 {
    rgb.iter().zip(LUMA).map(|(c, w)| c * w).sum()
}

/// The texel a toning dab writes over `current` (premultiplied) at
/// effective weight `k = amount * falloff`, or `None` to leave it (no
/// alpha, a non-finite sample or result). The alpha is `current`'s own.
fn toned_texel(op: ToneOp, current: &[f16], k: f32) -> Option<Texel> {
    let alpha = current.get(ALPHA)?;
    let a = alpha.to_f32();
    if !(a > 0.0 && a.is_finite()) {
        return None;
    }
    let mut straight = [0.0_f32; 3];
    for (slot, sample) in straight.iter_mut().zip(current) {
        let v = sample.to_f32() / a;
        if !v.is_finite() {
            return None;
        }
        *slot = v;
    }
    let y = luminance(straight);
    let mut out = [f16::ZERO; CHANNELS];
    for (slot, s) in out.iter_mut().zip(straight) {
        let moved = match op {
            ToneOp::Dodge(range) => s + (1.0 - s).max(0.0) * (k * range_weight(range, y)),
            ToneOp::Burn(range) => s - s.max(0.0) * (k * range_weight(range, y)),
            ToneOp::Desaturate => s + (y - s) * k,
            ToneOp::Saturate => (s + (s - y) * k).clamp(0.0, 1.0),
        };
        let premultiplied = moved * a;
        if !premultiplied.is_finite() {
            return None;
        }
        *slot = f16::from_f32(premultiplied);
    }
    out[ALPHA] = *alpha;
    Some(out)
}

/// One Dodge, Burn or Sponge dab (see the module docs): the Brush's soft
/// footprint at `center` (layer-local texels) toned by `amount` (`0..=1`,
/// clamped; NaN refuses the dab). `snapshot` is the stroke's undo snapshot
/// and captures what the dab writes; `coverage` restricts the dab to a
/// selection exactly as [`crate::stamp_dab_covered`]'s does.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
pub fn tone_dab_covered(
    store: &mut TileStore,
    surface: SurfaceId,
    center: (f32, f32),
    radius: f32,
    op: ToneOp,
    amount: f32,
    snapshot: Option<&mut StrokeSnapshot>,
    coverage: Option<&dyn Coverage>,
) -> DabOutcome {
    let mut outcome = DabOutcome::default();
    if amount.is_nan() || !is_paintable(center, radius) {
        return outcome;
    }
    if center.0.abs() > MAX_CENTER || center.1.abs() > MAX_CENTER {
        tracing::warn!(?center, "refusing a toning dab with an absurd centre");
        return outcome;
    }
    let side = 2 * (radius.ceil() as u64) + 3;
    let texels = side.saturating_mul(side);
    if texels > MAX_TONE_DAB_TEXELS {
        tracing::warn!(
            texels,
            max = MAX_TONE_DAB_TEXELS,
            "refusing a toning dab whose footprint is over the cap; it paints nothing"
        );
        return outcome;
    }
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return outcome;
    }

    let (cx, cy) = center;
    let min_x = (cx - radius).floor().max(0.0) as u32;
    let min_y = (cy - radius).floor().max(0.0) as u32;
    let max_x = (cx + radius).ceil().max(0.0) as u32;
    let max_y = (cy + radius).ceil().max(0.0) as u32;
    let (t0, t1) = tile_range(center, radius);
    outcome.painted = Vec::with_capacity(reserved_tiles(t0, t1));
    if max_x < min_x || max_y < min_y {
        return outcome;
    }

    // Everything the dab will touch must be readable before anything is
    // written: an unreadable tile refuses the whole dab.
    for ty in t0.y..=t1.y {
        for tx in t0.x..=t1.x {
            let id = TileId { x: tx, y: ty };
            if store.contains_tile(surface, id)
                && let Err(err) = store.get(surface, id)
            {
                outcome.fail(id, err);
                return outcome;
            }
        }
    }

    let mut snapshot = usable_snapshot(snapshot, surface);
    for ty in t0.y..=t1.y {
        for tx in t0.x..=t1.x {
            let id = TileId { x: tx, y: ty };
            // A tile the store does not hold is transparent everywhere:
            // nothing to tone, and no tile is made for it.
            if !store.contains_tile(surface, id) {
                continue;
            }
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
                    let index = texel_index(lx, ly);
                    let Some(current) = tile_texels.get(index..index + CHANNELS) else {
                        continue;
                    };
                    let Some(toned) = toned_texel(op, current, amount * a) else {
                        continue;
                    };
                    let fresh = if cover < 1.0 {
                        blend_texel(current, toned, cover)
                    } else {
                        toned
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
