//! The soft Brush's per-stroke accumulated coverage (0.230.0).
//!
//! **The model (a choice, not compared with Photoshop).** A stroke owns one
//! `f32` coverage `A` per texel, `0` where the stroke has not reached.
//! Every soft dab gives a texel the amount
//! `a = dab_falloff * flow * selection_coverage` and updates
//!
//! ```text
//! A' = A + (opacity - A) * a
//! ```
//!
//! so at Opacity 100 % and Flow 100 % this is `A' = 1 - (1 - A)(1 - a)`
//! (source-over build-up of the dabs) and **Opacity is the ceiling `A` can
//! reach**. The texel is then *recomputed from the stroke's start*, never
//! from the previous dab's stored value:
//!
//! ```text
//! stored = start * (1 - A') + colour * A'      (per premultiplied channel)
//! ```
//!
//! with `colour` premultiplied by alpha 1 (the foreground is opaque) and
//! `start` the texel as the stroke began it (the stroke snapshot's raw copy
//! of the tile; a tile the stroke has not written yet is still its own
//! start). That is source-over of the colour at alpha `A'` over the content
//! beneath, so a stroke blends with what it paints over instead of keeping
//! whichever of the two has the higher alpha (the max-alpha rule 0.230.0
//! removed). A texel is rewritten only if `A` changed; `A` is updated even
//! when the rounded `f16` would not change, so a faint dab (Flow 1 %) still
//! builds up.
//!
//! The transparency lock keeps the texel's alpha bit for bit and moves only
//! its colour with the same coverage `A'`:
//!
//! ```text
//! stored_c = start_c * (1 - A') + colour_c * alpha * A'
//! ```
//!
//! **Lifetime and size.** The coverage lives in the stroke's
//! [`crate::StrokeSnapshot`] (one 256 KiB `f32` tile per tile the stroke has
//! reached, allocated on the first texel that needs it) and is dropped when
//! [`crate::PixelHistory::push`] packs the snapshot, so it never reaches the
//! undo stack. With no snapshot a dab is its own stroke: `A` starts at 0 and
//! nothing is kept.
//!
//! The Pencil (a hard dab, `a` = 1) is the same family (`A' = opacity`) and
//! deliberately keeps its own, unchanged code.

use std::collections::HashMap;

use aurora_tile::{TILE, TileId};

/// Texels in one coverage tile.
pub(crate) const COVER_TEXELS: usize = (TILE * TILE) as usize;

/// One tile's coverage, row-major, `TILE * TILE` values.
pub(crate) type CoverTile = Box<[f32]>;

/// A fresh, all-zero coverage tile.
pub(crate) fn zeroed_tile() -> CoverTile {
    vec![0.0_f32; COVER_TEXELS].into_boxed_slice()
}

/// The index of tile-local texel `(x, y)` in a [`CoverTile`].
pub(crate) const fn cover_index(x: u32, y: u32) -> usize {
    (y * TILE + x) as usize
}

/// `A' = A + (opacity - A) * a`: the coverage after a dab of amount `a`.
/// Never below `a_old` for `0 <= a` and `a_old <= opacity`; callers treat a
/// result that does not exceed `a_old` as "nothing to do".
#[must_use]
pub(crate) fn accumulate(a_old: f32, opacity: f32, amount: f32) -> f32 {
    let next = a_old + (opacity - a_old) * amount;
    // Never past the ceiling (rounding can overshoot by an ulp); a NaN
    // amount adds nothing.
    if next.is_nan() {
        a_old
    } else {
        next.min(opacity)
    }
}

/// Every tile's coverage for one stroke.
#[derive(Debug, Clone, Default)]
pub(crate) struct StrokeCover {
    tiles: HashMap<TileId, CoverTile>,
}

impl StrokeCover {
    /// Takes `tile`'s coverage out (so the dab can borrow it mutably beside
    /// the snapshot's raw copy); [`Self::put`] returns it.
    pub(crate) fn take(&mut self, tile: TileId) -> Option<CoverTile> {
        self.tiles.remove(&tile)
    }

    /// Stores `tile`'s coverage.
    pub(crate) fn put(&mut self, tile: TileId, cover: CoverTile) {
        self.tiles.insert(tile, cover);
    }

    /// Drops everything (the stroke is over).
    pub(crate) fn clear(&mut self) {
        self.tiles = HashMap::new();
    }

    /// Tiles with coverage (a test and bound check).
    pub(crate) fn tile_count(&self) -> usize {
        self.tiles.len()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp, clippy::cast_precision_loss)]

    use super::accumulate;

    #[test]
    fn the_coverage_never_passes_its_ceiling_even_by_a_rounding_step() {
        // `a + (o - a) * 1` can land an `f32` ulp above `o`; the clamp holds.
        let mut overshoots_unclamped = 0;
        for i in 0..400_u32 {
            for j in 0..400_u32 {
                let (a, o) = (i as f32 / 401.0, (j as f32 + 1.0) / 400.0);
                if a >= o {
                    continue;
                }
                let got = accumulate(a, o, 1.0);
                assert!(got <= o, "a {a} o {o}: {got}");
                if a + (o - a) > o {
                    overshoots_unclamped += 1;
                }
            }
        }
        assert!(
            overshoots_unclamped > 0,
            "the sweep must contain a rounding overshoot"
        );
    }

    #[test]
    fn accumulate_is_the_documented_recurrence() {
        assert_eq!(accumulate(0.0, 1.0, 0.5), 0.5);
        assert_eq!(accumulate(0.5, 1.0, 0.5), 0.75);
        assert_eq!(accumulate(0.25, 0.5, 0.5), 0.375);
        assert_eq!(
            accumulate(0.5, 0.5, 0.9),
            0.5,
            "at the ceiling nothing is added"
        );
        assert_eq!(
            accumulate(0.3, 1.0, f32::NAN),
            0.3,
            "a NaN amount adds nothing"
        );
    }
}
