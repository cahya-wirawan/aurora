//! The composite pyramid: one 2 x 2 box-filter step from four tiles of a
//! level to the one tile of the next level that covers them.
//!
//! Level 0 is the document composite itself (the tiles the canvas shows at
//! 100 %); level `L` tile `(x, y)` covers the document rectangle
//! `[x, x + 1) * TILE * 2^L` by `[y, y + 1) * TILE * 2^L`, and each of its
//! texels is the mean of the 2 x 2 texels of level `L - 1` under it. That
//! makes level `L` exactly "the composite scaled down by a `2^L` box filter",
//! the export at that size, whatever the layers' blend modes, adjustments
//! or effects are (it filters the finished picture, not the layers).
//!
//! **Filtered in premultiplied space.** Each input texel is premultiplied
//! (`rgb * a`) in `f32`, the four are summed and divided by four, and the
//! result is stored straight (`rgb / a`) as `f16`, the store's convention,
//! so the atlas upload premultiplies it again exactly as it does a level-0
//! tile. A transparent texel therefore weighs nothing in the colour, which
//! is what keeps a hard edge from growing a dark halo.
//!
//! **The document border.** `valid` gives, per axis, how many texels of the
//! *input* level counted from the block's own input-level origin lie inside
//! the document. Texels at or past it are treated as fully transparent, so
//! layer content that reaches past the canvas (never shown at 100 % either,
//! the canvas shader clips it) does not bleed into the border texels. The
//! last texel row or column of a document that is not a multiple of `2^L`
//! is the mean over the full block with the outside counted as transparent:
//! the box filter of the export padded with transparency.
//!
//! **Non-finite input.** A texel with any non-finite channel contributes
//! nothing (it is treated as transparent). The 100 % view draws such a texel
//! however the GPU treats it; the pyramid has to pick something
//! deterministic, and a NaN propagated through a mean would spread over a
//! whole block at every level.

use aurora_tile::{CHANNELS, SAMPLES, TILE};
use half::f16;

/// Texels per row of a tile.
const ROW: usize = TILE as usize;
/// Samples per row of a tile.
const ROW_SAMPLES: usize = ROW * CHANNELS;
/// Output texels each input quadrant fills per row.
const HALF: usize = ROW / 2;

/// One input texel premultiplied, or zero when it is outside the document
/// or carries a non-finite value.
#[inline]
fn premultiplied(texel: &[f16], inside: bool) -> [f32; 4] {
    if !inside {
        return [0.0; 4];
    }
    let [r, g, b, a] = texel else {
        return [0.0; 4];
    };
    let (r, g, b, a) = (r.to_f32(), g.to_f32(), b.to_f32(), a.to_f32());
    if !(r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite()) {
        return [0.0; 4];
    }
    [r * a, g * a, b * a, a]
}

/// Builds the tile of the next pyramid level from the four tiles under it.
///
/// `children` is `[top-left, top-right, bottom-left, bottom-right]`; `None`
/// (or a slice that is not exactly one tile) is a transparent tile. `valid`
/// is the number of input-level texels inside the document from the block's
/// input-level origin, per axis (`(2 * TILE, 2 * TILE)` or more when the
/// whole block is inside). `out` must be one tile ([`SAMPLES`] long); it is
/// written in full (straight alpha).
///
/// Deterministic: the same inputs give the same bits on every call and
/// every thread (plain `f32` arithmetic in a fixed order).
pub fn downsample_quad(children: [Option<&[f16]>; 4], valid: (u32, u32), out: &mut [f16]) {
    if out.len() != SAMPLES {
        return;
    }
    let valid = (valid.0 as usize, valid.1 as usize);
    for (quadrant, child) in children.into_iter().enumerate() {
        let (cx, cy) = (quadrant % 2, quadrant / 2);
        let child = child.filter(|texels| texels.len() == SAMPLES);
        // Output rows `cy * HALF ..`, columns `cx * HALF ..`.
        let out_rows = out
            .chunks_exact_mut(ROW_SAMPLES)
            .skip(cy * HALF)
            .take(HALF);
        let Some(child) = child else {
            for row in out_rows {
                if let Some(part) = row.get_mut(cx * HALF * CHANNELS..(cx + 1) * HALF * CHANNELS) {
                    part.fill(f16::ZERO);
                }
            }
            continue;
        };
        for (oy, (pair, out_row)) in child
            .chunks_exact(2 * ROW_SAMPLES)
            .zip(out_rows)
            .enumerate()
        {
            let (top, bottom) = pair.split_at(ROW_SAMPLES);
            let y0 = cy * ROW + 2 * oy;
            let inside_top = y0 < valid.1;
            let inside_bottom = y0 + 1 < valid.1;
            let Some(out_part) =
                out_row.get_mut(cx * HALF * CHANNELS..(cx + 1) * HALF * CHANNELS)
            else {
                continue;
            };
            for (ox, ((t, b), o)) in top
                .chunks_exact(2 * CHANNELS)
                .zip(bottom.chunks_exact(2 * CHANNELS))
                .zip(out_part.chunks_exact_mut(CHANNELS))
                .enumerate()
            {
                let x0 = cx * ROW + 2 * ox;
                let left = x0 < valid.0;
                let right = x0 + 1 < valid.0;
                let (t0, t1) = t.split_at(CHANNELS);
                let (b0, b1) = b.split_at(CHANNELS);
                let p = [
                    premultiplied(t0, inside_top && left),
                    premultiplied(t1, inside_top && right),
                    premultiplied(b0, inside_bottom && left),
                    premultiplied(b1, inside_bottom && right),
                ];
                let mut sum = [0.0_f32; 4];
                for texel in p {
                    for (s, v) in sum.iter_mut().zip(texel) {
                        *s += v;
                    }
                }
                let straight = if sum[3] > 0.0 {
                    [
                        sum[0] / sum[3],
                        sum[1] / sum[3],
                        sum[2] / sum[3],
                        sum[3] * 0.25,
                    ]
                } else {
                    [0.0; 4]
                };
                for (slot, value) in o.iter_mut().zip(straight) {
                    *slot = f16::from_f32(value);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "pyramid_tests.rs"]
mod tests;
