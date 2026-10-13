//! The Blur and Sharpen tools' dab (0.223.0): one shared engine that
//! softens or sharpens the layer's own pixels under the Brush's soft
//! footprint.
//!
//! **What a dab does**, with `s` the strength in `0..=1` and `a =
//! dab_falloff(d, radius)` the Brush's falloff at distance `d` from the
//! centre (exactly the texels [`crate::stamp_dab`] visits), `k = s * a`:
//!
//! 1. *Read.* The layer is read once, as it is now, into a premultiplied
//!    `f32` patch covering the footprint plus a one-texel border (an
//!    absent tile, or a texel left of or above the origin, is transparent;
//!    a non-finite sample counts as `0`). Nothing is written until the
//!    whole patch is read, so every footprint texel sees the layer as it
//!    was **before this dab** and the result does not depend on the order
//!    the texels are visited in. The layer is the stroke's own current
//!    state: later dabs see earlier dabs' output, like the Smudge.
//! 2. *Average.* For each footprint texel `avg` is the 3 x 3 weighted mean
//!    of the patch around it, the separable `[1 2 1] x [1 2 1] / 16`
//!    kernel (so the centre weighs 4/16 and the corners 1/16), per
//!    premultiplied channel, alpha included. The kernel's size is fixed
//!    (one texel), whatever the brush radius.
//! 3. *Blur* moves the texel toward its average, `new = old + (avg - old) * k`.
//!    At `k = 1` the texel is its average; the result is a convex mix of
//!    texels already in the layer, so it needs no clamping.
//! 4. *Sharpen* is an unsharp mask: `new = old + (old - avg) * G * k`,
//!    with the gain `G` = [`SHARPEN_GAIN`] (2). It then clamps the alpha
//!    to `0..=1` and every colour channel to `0..=new alpha`, so the
//!    result is always a valid premultiplied texel (never colour above
//!    alpha) and never overshoots past the representable range. A
//!    sharpened flat area is unchanged (`avg == old`).
//!
//! Coverage (a selection) then blends `old` toward `new` by the coverage,
//! as every other tool does.
//!
//! A **transparency lock** (`preserve_alpha`) keeps each destination
//! texel's alpha bit for bit and works on straight colour: the average's
//! straight colour is `avg.rgb / avg.a` (the alpha-weighted mean of the
//! neighbours' colours), the texel's own is `old.rgb / old.a`, the same
//! blur or sharpen formula runs on the straight colours (sharpen clamped
//! to `0..=1`) and the result is re-premultiplied by the texel's own alpha.
//! A texel with no alpha is left alone.
//!
//! All of this is **a choice, not compared with Photoshop's Blur and
//! Sharpen**: Photoshop's kernels, their dependence on the brush size and
//! the strength curve are not modelled, there is no "Sample All Layers",
//! and the layer's own edges (left of and above the origin, and wherever a
//! tile is absent) read as transparent, so blurring pulls an edge's alpha
//! out into the empty side.
//!
//! **Bounds and failure.** The patch never holds more than
//! [`MAX_FILTER_PATCH_TEXELS`]: a dab over that, a centre beyond `2^31`
//! texels, or a strength that is not a number is refused whole and touches
//! nothing. A strength of `0` changes nothing. An unreadable tile refuses
//! the whole dab before anything is written (reported in
//! [`DabOutcome::failed`]).
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
use crate::smudge::gather_under;
use crate::stamp::{
    ALPHA, DabOutcome, Texel, is_paintable, malformed_tile, reserved_tiles, stores_the_same_bits,
    texel_alpha, texel_index, tile_coverage, tile_range, usable_snapshot,
};

/// The most texels the patch may hold (1024 x 1024, a radius of about
/// 509): an engineering default that bounds it to 16 MiB of `f32`. A larger
/// dab is refused whole.
pub const MAX_FILTER_PATCH_TEXELS: u64 = 1 << 20;

/// The unsharp mask's gain `G` (see the module docs): at full strength and
/// full falloff a texel moves twice the way from its 3 x 3 average that it
/// already is. A choice: the `[1 2 1]` average is a mild blur, so a gain of
/// 1 would barely show.
pub const SHARPEN_GAIN: f32 = 2.0;

/// The largest centre coordinate magnitude a dab accepts (`2^31` texels).
const MAX_CENTER: f32 = 2_147_483_648.0;

/// The separable kernel's weights; the 2-D weight is the product and the
/// sum of all nine is [`KERNEL_SUM`].
const KERNEL: [f32; 3] = [1.0, 2.0, 1.0];

/// `(1 + 2 + 1)^2`.
const KERNEL_SUM: f32 = 16.0;

/// Which of the two brushes a dab is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    /// Moves texels toward their local average.
    Blur,
    /// Moves texels away from their local average (an unsharp mask).
    Sharpen,
}

/// The layer under a dab, read before it writes: `side x side` premultiplied
/// RGBA `f32` texels whose top-left texel is layer texel `(x0, y0)`.
struct Patch {
    texels: Vec<f32>,
    side: i64,
    x0: i64,
    y0: i64,
}

impl Patch {
    /// The texel at layer position `(lx, ly)`, transparent outside the patch.
    fn texel(&self, lx: i64, ly: i64) -> [f32; CHANNELS] {
        let (px, py) = (lx - self.x0, ly - self.y0);
        if px < 0 || py < 0 || px >= self.side || py >= self.side {
            return [0.0; CHANNELS];
        }
        let from = usize::try_from(py * self.side + px).map_or(0, |i| i * CHANNELS);
        match self.texels.get(from..from + CHANNELS) {
            Some([red, green, blue, alpha]) => [*red, *green, *blue, *alpha],
            _ => [0.0; CHANNELS],
        }
    }

    /// The `[1 2 1] x [1 2 1] / 16` weighted mean around `(x, y)`.
    fn average(&self, x: i64, y: i64) -> [f32; CHANNELS] {
        let mut sum = [0.0_f32; CHANNELS];
        for (j, wy) in (-1_i64..=1).zip(KERNEL) {
            for (i, wx) in (-1_i64..=1).zip(KERNEL) {
                let t = self.texel(x + i, y + j);
                for (s, v) in sum.iter_mut().zip(t) {
                    *s += wx * wy * v;
                }
            }
        }
        sum.map(|s| s / KERNEL_SUM)
    }
}

/// The texel a dab writes over a texel that reads `old` (premultiplied)
/// with local average `avg`, weighted by `k = s * a` (see the module docs),
/// or `None` to leave it. `kept_alpha` is the texel's stored alpha, used
/// only with a transparency lock.
fn filtered_texel(
    kind: FilterKind,
    old: [f32; CHANNELS],
    avg: [f32; CHANNELS],
    k: f32,
    kept_alpha: Option<f16>,
) -> Option<Texel> {
    // `delta = (avg - old) * toward`: blur goes toward the average, sharpen
    // away from it, by the gain.
    let toward = match kind {
        FilterKind::Blur => 1.0,
        FilterKind::Sharpen => -SHARPEN_GAIN,
    };
    let at = |v: &[f32; CHANNELS], c: usize| v.get(c).copied().unwrap_or(0.0);
    if let Some(kept) = kept_alpha {
        let (dst_alpha, avg_alpha) = (at(&old, ALPHA), at(&avg, ALPHA));
        if dst_alpha <= 0.0 || avg_alpha <= 0.0 {
            return None;
        }
        let colour = |c: usize| {
            let own = at(&old, c) / dst_alpha;
            let mean = at(&avg, c) / avg_alpha;
            let moved = own + (mean - own) * toward * k;
            let moved = match kind {
                FilterKind::Blur => moved,
                FilterKind::Sharpen => moved.clamp(0.0, 1.0),
            };
            moved * dst_alpha
        };
        let (r, g, b) = (colour(0), colour(1), colour(2));
        if !(r.is_finite() && g.is_finite() && b.is_finite()) {
            return None;
        }
        return Some([f16::from_f32(r), f16::from_f32(g), f16::from_f32(b), kept]);
    }
    let mut out = [0.0_f32; CHANNELS];
    for (c, slot) in out.iter_mut().enumerate() {
        let (o, m) = (at(&old, c), at(&avg, c));
        *slot = o + (m - o) * toward * k;
    }
    // Checked before the clamps: `f32::clamp` panics on a NaN bound.
    if !out.iter().all(|v| v.is_finite()) {
        return None;
    }
    if kind == FilterKind::Sharpen {
        let alpha = out[ALPHA].clamp(0.0, 1.0);
        for slot in &mut out {
            *slot = slot.clamp(0.0, alpha);
        }
        out[ALPHA] = alpha;
    }
    Some(out.map(f16::from_f32))
}

/// What a dab writes over `current` at layer texel `(x, y)`, footprint
/// weight `a` and selection coverage `cover`, or `None` to leave the texel.
#[allow(clippy::too_many_arguments)]
fn written_texel(
    kind: FilterKind,
    patch: &Patch,
    (x, y): (i64, i64),
    current: &[f16],
    strength: f32,
    a: f32,
    cover: f32,
    preserve_alpha: bool,
) -> Option<Texel> {
    let old = patch.texel(x, y);
    let avg = patch.average(x, y);
    let kept = if preserve_alpha {
        Some(current.get(ALPHA).copied()?)
    } else {
        None
    };
    let fresh = filtered_texel(kind, old, avg, strength * a, kept)?;
    Some(if cover < 1.0 {
        blend_texel(current, fresh, cover)
    } else {
        fresh
    })
}

/// Whether any texel of `tile_box` would change in a tile the store does
/// not hold (every sample `0`): some covered footprint texel whose result
/// is not transparent black. Only consulted for such a tile, so blurring or
/// sharpening empty space creates no tile.
#[allow(clippy::too_many_arguments, clippy::cast_precision_loss)]
fn reaches_blank_tile(
    kind: FilterKind,
    patch: &Patch,
    center: (f32, f32),
    radius: f32,
    strength: f32,
    preserve_alpha: bool,
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
            let cover = coverage.map_or(1.0, |cov| covered(cov, i64::from(x), i64::from(y)));
            if cover <= 0.0 {
                continue;
            }
            if let Some(fresh) = written_texel(
                kind,
                patch,
                (i64::from(x), i64::from(y)),
                &zero,
                strength,
                a,
                cover,
                false,
            ) && !stores_the_same_bits(&zero, fresh)
            {
                return true;
            }
        }
    }
    false
}

/// One Blur or Sharpen dab (see the module docs): the Brush's soft
/// footprint at `center` (layer-local texels) blurred or sharpened by
/// `strength` (`0..=1`, clamped; NaN refuses the dab). `snapshot` is the
/// stroke's undo snapshot and captures what the dab writes; `coverage`
/// restricts the dab to a selection exactly as [`crate::stamp_dab_covered`]'s
/// does; `preserve_alpha` is the transparency lock.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
pub fn filter_dab_covered(
    store: &mut TileStore,
    surface: SurfaceId,
    center: (f32, f32),
    radius: f32,
    kind: FilterKind,
    strength: f32,
    preserve_alpha: bool,
    snapshot: Option<&mut StrokeSnapshot>,
    coverage: Option<&dyn Coverage>,
) -> DabOutcome {
    let mut outcome = DabOutcome::default();
    if strength.is_nan() || !is_paintable(center, radius) {
        return outcome;
    }
    if center.0.abs() > MAX_CENTER || center.1.abs() > MAX_CENTER {
        tracing::warn!(
            ?center,
            "refusing a blur or sharpen dab with an absurd centre"
        );
        return outcome;
    }
    // The footprint lies within the anchor's `ceil(radius) + 1` neighbours;
    // the averaging border adds one more.
    let half = radius.ceil() as i64 + 2;
    let side = 2 * half + 1;
    let texels = (side as u64).saturating_mul(side as u64);
    if texels > MAX_FILTER_PATCH_TEXELS {
        tracing::warn!(
            texels,
            max = MAX_FILTER_PATCH_TEXELS,
            "refusing a blur or sharpen dab whose patch is over the cap; it paints nothing"
        );
        return outcome;
    }
    let strength = strength.clamp(0.0, 1.0);
    if strength <= 0.0 {
        return outcome;
    }
    let anchor = (center.0.floor() as i64, center.1.floor() as i64);

    // Everything the dab needs is read before anything is written.
    let texels = match gather_under(store, surface, anchor, half) {
        Ok(texels) => texels,
        Err((id, err)) => {
            outcome.fail(id, err);
            return outcome;
        }
    };
    let patch = Patch {
        texels,
        side,
        x0: anchor.0 - half,
        y0: anchor.1 - half,
    };

    let mut snapshot = usable_snapshot(snapshot, surface);
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
                    kind,
                    &patch,
                    center,
                    radius,
                    strength,
                    preserve_alpha,
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
                    let index = texel_index(lx, ly);
                    let Some(current) = tile_texels.get(index..index + CHANNELS) else {
                        continue;
                    };
                    let Some(fresh) = written_texel(
                        kind,
                        &patch,
                        (i64::from(x), i64::from(y)),
                        current,
                        strength,
                        a,
                        cover,
                        preserve_alpha,
                    ) else {
                        continue;
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
