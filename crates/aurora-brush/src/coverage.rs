//! Painting through a selection (0.211.0): the [`Coverage`] abstraction.
//!
//! A tool computes the value it would write exactly as it always did and
//! then blends it in by the **coverage** at that texel:
//! `result = old + (new - old) * coverage`, per premultiplied `f16`
//! channel (the Eraser scales its erase amount by the coverage instead,
//! which is the same thing). Coverage `1` is today's output bit for bit
//! (the blend is skipped, not merely close), coverage `0` leaves the
//! texel, and a tile with no covered texel, untouched and uncreated.
//!
//! **Why a trait, and why here.** The selection lives in `aurora-doc`,
//! which this crate cannot see (layering: downward only), and the
//! selections still to come (Lasso, Magic Wand, feathered or antialiased
//! edges) are rasters, not rectangles. The tools therefore talk to this
//! trait and nothing else: a rectangle gives exactly `0` or `1`, a raster
//! gives anything between, and no tool changes when one arrives.
//!
//! **Space.** Coordinates are those of the surface being painted
//! (layer-local texels, `(0, 0)` the layer's top-left). The caller
//! converts from document space; this crate never sees a document.
//!
//! **Cost.** `at` must be O(1). It is only called for texels a tool is
//! about to write, never when no selection is active (the tools take an
//! `Option`), and [`Coverage::region`] lets a whole tile be skipped
//! (`Empty`) or run without per-texel lookups (`Full`).

use aurora_core::Rect;
use aurora_tile::CHANNELS;
use half::f16;

/// How a rectangle of the surface relates to a [`Coverage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageRegion {
    /// Coverage is exactly `0` everywhere in the rectangle.
    Empty,
    /// Coverage is exactly `1` everywhere in the rectangle.
    Full,
    /// Anything else, or not known: ask [`Coverage::at`] per texel.
    Partial,
}

/// Per-texel coverage in `0..=1` of the area a tool may change.
pub trait Coverage {
    /// A rectangle outside which [`Self::at`] is `0`, or `None` when there
    /// is no useful bound (an inverted selection). Used to early-out and
    /// to keep a fill from visiting tiles it cannot change.
    fn bounds(&self) -> Option<Rect>;

    /// The coverage at surface texel `(x, y)`, in `0.0..=1.0` (anything
    /// else is clamped; NaN counts as `0`).
    fn at(&self, x: i64, y: i64) -> f32;

    /// Classifies `rect`. The default is correct for any implementation
    /// that honours [`Self::bounds`]: a rectangle disjoint from the bounds
    /// is `Empty`, anything else `Partial`. An implementation that knows
    /// more (a rectangle selection knows when a tile lies wholly inside)
    /// overrides it to skip per-texel lookups.
    fn region(&self, rect: Rect) -> CoverageRegion {
        match self.bounds() {
            Some(bounds) if !bounds.intersects(&rect) => CoverageRegion::Empty,
            _ => CoverageRegion::Partial,
        }
    }
}

/// The coverage at `(x, y)`, clamped to `0..=1` (NaN is `0`).
#[must_use]
pub(crate) fn covered(coverage: &dyn Coverage, x: i64, y: i64) -> f32 {
    let c = coverage.at(x, y);
    if c > 0.0 { c.min(1.0) } else { 0.0 }
}

/// `old + (new - old) * c` per channel, rounded to `f16`. The caller has
/// already handled `c >= 1` (use `new` as is) and `c <= 0` (leave `old`).
/// A channel whose new and old values are the same bits stays the same
/// bits, so a fractional blend never nudges an alpha it should not touch.
#[must_use]
pub(crate) fn blend_texel(old: &[f16], new: [f16; CHANNELS], c: f32) -> [f16; CHANNELS] {
    let mut out = new;
    for (channel, slot) in out.iter_mut().enumerate() {
        let Some(&before) = old.get(channel) else {
            continue;
        };
        if before.to_bits() == slot.to_bits() {
            continue;
        }
        let (o, n) = (before.to_f32(), slot.to_f32());
        *slot = f16::from_f32(o + (n - o) * c);
    }
    out
}

/// A texel, if `c` says to write it at all: `new` at full coverage, the
/// blend in between, `None` for none.
#[must_use]
pub(crate) fn apply_coverage(old: &[f16], new: [f16; CHANNELS], c: f32) -> Option<[f16; CHANNELS]> {
    if c >= 1.0 {
        Some(new)
    } else if c > 0.0 {
        Some(blend_texel(old, new, c))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_is_exact_at_the_ends_and_a_lerp_between() {
        let old = [f16::from_f32(0.0); 4];
        let new = [f16::from_f32(1.0); 4];
        assert_eq!(apply_coverage(&old, new, 1.0), Some(new));
        assert_eq!(apply_coverage(&old, new, 2.0), Some(new));
        assert_eq!(apply_coverage(&old, new, 0.0), None);
        assert_eq!(apply_coverage(&old, new, f32::NAN), None);
        assert_eq!(
            apply_coverage(&old, new, 0.5),
            Some([f16::from_f32(0.5); 4])
        );
        // The direction matters: coverage weighs the NEW value.
        assert_eq!(
            apply_coverage(&new, old, 0.25),
            Some([f16::from_f32(0.75); 4])
        );
    }

    #[test]
    fn an_unchanged_channel_keeps_its_bits() {
        let old = [
            f16::from_f32(0.3),
            f16::from_f32(0.3),
            f16::from_f32(0.3),
            f16::from_f32(0.7),
        ];
        let mut new = old;
        new[0] = f16::from_f32(0.9);
        let out = apply_coverage(&old, new, 0.37);
        assert_eq!(out.map(|t| t[3].to_bits()), Some(old[3].to_bits()));
        assert_eq!(out.map(|t| t[1].to_bits()), Some(old[1].to_bits()));
    }

    struct Everything;
    impl Coverage for Everything {
        fn bounds(&self) -> Option<Rect> {
            None
        }
        fn at(&self, _: i64, _: i64) -> f32 {
            1.0
        }
    }

    #[test]
    fn the_default_region_follows_the_bounds() {
        struct Bounded;
        impl Coverage for Bounded {
            fn bounds(&self) -> Option<Rect> {
                Some(Rect {
                    x: 10,
                    y: 10,
                    width: 5,
                    height: 5,
                })
            }
            fn at(&self, _: i64, _: i64) -> f32 {
                1.0
            }
        }
        let r = |x, y| Rect {
            x,
            y,
            width: 4,
            height: 4,
        };
        assert_eq!(Bounded.region(r(0, 0)), CoverageRegion::Empty);
        assert_eq!(Bounded.region(r(14, 14)), CoverageRegion::Partial);
        assert_eq!(Bounded.region(r(15, 15)), CoverageRegion::Empty);
        assert_eq!(Everything.region(r(0, 0)), CoverageRegion::Partial);
        assert_eq!(covered(&Everything, 0, 0), 1.0);
    }
}
