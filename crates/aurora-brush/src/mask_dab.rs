//! A dab on a plain 8-bit grid (0.217.0, Quick Mask): the layer brush's
//! footprint and falloff applied to a dense coverage buffer instead of a
//! layer's tiles.
//!
//! The grid is `width x height` bytes, row-major, with texel `(x, y)`
//! covering the document square `[x, x + 1) x [y, y + 1)`; a dab's centre
//! and radius are in the same (document) units as the layer brush's, and a
//! texel is in the footprint exactly when the layer brush would paint it:
//! its centre `(x + 0.5, y + 0.5)` lies within `radius` of the dab centre
//! and [`dab_falloff`] is positive there. [`dab_falloff`] is the one
//! formula both the layer brush (`stamp.rs`) and this module use, so the
//! two cannot drift apart.
//!
//! **Cost.** The loop visits the dab's bounding box clipped to the grid, so
//! it is bounded by the grid however large the radius.

use aurora_core::Rect;

/// The dab's alpha at distance `d` from its centre: `(1 - d / radius)^1.5`
/// clamped to `0..=1` (`0` at and beyond the radius). The layer brush's
/// falloff; hardness is not a variable.
#[must_use]
pub fn dab_falloff(d: f32, radius: f32) -> f32 {
    (1.0 - (d / radius)).clamp(0.0, 1.0).powf(1.5)
}

/// Moves every texel in the dab's footprint toward `target` by the dab's
/// falloff `a`: `new = round(old + (target - old) * a)`. A texel already at
/// `target` is unchanged (so a second identical dab changes nothing), and
/// the result always lies between `old` and `target`, so it never leaves
/// `0..=255`.
///
/// Returns the smallest rectangle holding every texel whose byte changed,
/// or `None` when nothing did: a non-finite centre or radius, a radius of
/// zero or less, a dab off the grid, a grid whose `data` is not
/// `width * height` bytes, or a footprint of texels that already read
/// `target` (or whose change rounds to nothing).
pub fn stamp_toward(
    data: &mut [u8],
    width: u32,
    height: u32,
    center: (f32, f32),
    radius: f32,
    target: u8,
) -> Option<Rect> {
    if !(center.0.is_finite() && center.1.is_finite() && radius.is_finite() && radius > 0.0) {
        return None;
    }
    let (w, h) = (usize::try_from(width).ok()?, usize::try_from(height).ok()?);
    if data.len() != w.checked_mul(h)? || w == 0 || h == 0 {
        return None;
    }
    let clip = |lo: f32, hi: f32, limit: u32| -> Option<(u32, u32)> {
        // Texels whose centre can lie within `radius`: floor/ceil of the
        // span, clamped to the grid. Float-to-int casts saturate.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let first = (lo - 0.5).floor().max(0.0) as u32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let last = (hi - 0.5).ceil().max(-1.0).min(limit as f32) as i64;
        let last = u32::try_from(last).ok()?.min(limit.checked_sub(1)?);
        (first <= last).then_some((first, last))
    };
    let (x0, x1) = clip(center.0 - radius, center.0 + radius, width)?;
    let (y0, y1) = clip(center.1 - radius, center.1 + radius, height)?;
    let mut changed: Option<(u32, u32, u32, u32)> = None;
    for y in y0..=y1 {
        #[allow(clippy::cast_precision_loss)]
        let py = y as f32 + 0.5;
        let row = usize::try_from(y).ok()?.checked_mul(w)?;
        for x in x0..=x1 {
            #[allow(clippy::cast_precision_loss)]
            let px = x as f32 + 0.5;
            let d = (px - center.0).hypot(py - center.1);
            if d > radius {
                continue;
            }
            let a = dab_falloff(d, radius);
            if a <= 0.0 {
                continue;
            }
            let Some(slot) = data.get_mut(row + usize::try_from(x).ok()?) else {
                continue;
            };
            let old = f32::from(*slot);
            let moved = old + (f32::from(target) - old) * a;
            // Between `old` and `target`, both bytes, so in 0..=255.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let new = moved.round().clamp(0.0, 255.0) as u8;
            if new == *slot {
                continue;
            }
            *slot = new;
            changed = Some(match changed {
                None => (x, y, x, y),
                Some((a0, b0, a1, b1)) => (a0.min(x), b0.min(y), a1.max(x), b1.max(y)),
            });
        }
    }
    changed.map(|(a0, b0, a1, b1)| Rect {
        x: i64::from(a0),
        y: i64::from(b0),
        width: a1 - a0 + 1,
        height: b1 - b0 + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::{Rect, dab_falloff, stamp_toward};

    const ZERO: Rect = Rect {
        x: -1,
        y: -1,
        width: 0,
        height: 0,
    };

    fn grid(w: u32, h: u32, v: u8) -> Vec<u8> {
        vec![v; (w * h) as usize]
    }

    fn at(data: &[u8], w: u32, x: u32, y: u32) -> u8 {
        data.get((y * w + x) as usize).copied().unwrap_or(0)
    }

    #[test]
    fn falloff_is_one_at_the_centre_and_zero_at_and_beyond_the_radius() {
        assert!((dab_falloff(0.0, 10.0) - 1.0).abs() < 1e-6);
        assert!(dab_falloff(10.0, 10.0).abs() < 1e-6);
        assert!(dab_falloff(11.0, 10.0).abs() < 1e-6);
        // (1 - 0.5)^1.5
        assert!((dab_falloff(5.0, 10.0) - 0.353_553_4).abs() < 1e-5);
    }

    #[test]
    fn a_dab_moves_the_centre_texel_all_the_way_and_the_edge_partway() {
        let mut data = grid(40, 40, 0);
        // Centre on a texel centre so that texel has d = 0.
        let r = stamp_toward(&mut data, 40, 40, (20.5, 20.5), 8.0, 255);
        assert_eq!(at(&data, 40, 20, 20), 255);
        let edge = at(&data, 40, 27, 20);
        assert!(edge > 0 && edge < 255, "edge {edge}");
        assert_eq!(at(&data, 40, 29, 20), 0, "outside the radius");
        let r = r.unwrap_or(ZERO);
        assert_eq!((r.x, r.y), (13, 13), "dirty rect starts at the footprint");
        assert!(r.width <= 16 && r.height <= 16);
    }

    #[test]
    fn a_hard_target_value_is_a_fixed_point_and_never_overshoots() {
        let mut data = grid(32, 32, 200);
        assert_eq!(
            stamp_toward(&mut data, 32, 32, (16.0, 16.0), 6.0, 200),
            None
        );
        assert!(data.iter().all(|&v| v == 200));
        // Toward 0 from 200: monotone down, never below 0, and it
        // converges (rounding stalls the faintest edge texels).
        let mut passes = 0;
        while stamp_toward(&mut data, 32, 32, (16.0, 16.0), 6.0, 0).is_some() {
            passes += 1;
            assert!(passes < 1000, "must converge");
        }
        assert!(data.iter().all(|&v| v <= 200));
        assert_eq!(at(&data, 32, 15, 15), 0);
        // A further identical dab on the converged spot changes nothing.
        assert_eq!(stamp_toward(&mut data, 32, 32, (16.0, 16.0), 6.0, 0), None);
    }

    #[test]
    fn dabs_off_the_grid_or_with_bad_input_change_nothing() {
        let mut data = grid(16, 16, 10);
        for (c, r) in [
            ((-50.0, 5.0), 4.0),
            ((5.0, 500.0), 4.0),
            ((f32::NAN, 5.0), 4.0),
            ((5.0, 5.0), f32::NAN),
            ((5.0, 5.0), 0.0),
            ((5.0, 5.0), -3.0),
            ((5.0, 5.0), f32::INFINITY),
        ] {
            assert_eq!(
                stamp_toward(&mut data, 16, 16, c, r, 255),
                None,
                "{c:?} {r}"
            );
        }
        assert!(data.iter().all(|&v| v == 10));
        // A grid of the wrong length.
        let mut short = vec![0_u8; 10];
        assert_eq!(stamp_toward(&mut short, 16, 16, (5.0, 5.0), 4.0, 255), None);
    }

    #[test]
    fn a_dab_straddling_the_edge_is_clipped_not_wrapped() {
        let mut data = grid(16, 16, 0);
        let r = stamp_toward(&mut data, 16, 16, (0.0, 0.0), 5.0, 255).unwrap_or(ZERO);
        assert_eq!((r.x, r.y), (0, 0));
        assert!(at(&data, 16, 0, 0) > 0);
        assert_eq!(at(&data, 16, 15, 0), 0, "no wrap to the far side");
        assert_eq!(at(&data, 16, 0, 15), 0);
    }

    #[test]
    fn a_huge_radius_is_bounded_by_the_grid() {
        let mut data = grid(8, 8, 0);
        let r = stamp_toward(&mut data, 8, 8, (4.0, 4.0), 1.0e9, 255).unwrap_or(ZERO);
        assert!(r.width <= 8 && r.height <= 8);
    }
}
