//! Pen pressure for the Brush, Pencil and Eraser (0.308.0): the pure maths.
//!
//! The pen sample's pressure (`aurora_core::PenSample::pressure`, 0..=1, a
//! mouse is 1.0) reaches a dab through these helpers; `App` and the free
//! stamp helpers only call them.
//!
//! **Size.** The dab radius is `max(min_radius, base * p)` with
//! `min_radius = min(1 px, base)`: a pen at zero pressure still leaves a
//! one-pixel mark (the radius slider's own floor), and the radius is never
//! above the slider's value.
//!
//! **Opacity.** The stroke's Opacity is a per-stroke ceiling
//! (`aurora_brush::PaintAmount`); with pressure for opacity the ceiling used
//! by one dab is `opacity * p`. Every tool's model keeps coverage monotone
//! under a varying ceiling: the soft Brush's `A' = A + (ceiling - A) * a`
//! writes nothing when `A` is already above the dab's ceiling (so a light
//! tail never lightens what a heavy middle laid down, and the stroke never
//! passes the user's Opacity), the Eraser's floor `start * (1 - ceiling)`
//! only ever lowers alpha, and the Pencil skips a texel whose limited alpha
//! would be lower than the one already there. The Pencil's transparency-lock
//! mode (a colour move toward a limited target) is not monotone and can
//! lighten an overlap by a lighter dab.
//!
//! **Between events.** Pressure is interpolated linearly along each segment
//! between two pointer events by the dab's distance from the segment start.
//! Dab spacing uses the pressured radius at the segment's mean pressure
//! (Photoshop spaces by the current diameter), floored by `dab_step`'s
//! 0.5 px step so a tiny pressure cannot multiply the dab count.

use aurora_brush::PaintAmount;

/// The smallest dab radius pressure can give, in document pixels.
pub(crate) const MIN_PRESSURE_RADIUS_PX: f32 = 1.0;

/// A pressure in `0..=1`; a non-finite value reads as full pressure.
#[must_use]
pub(crate) fn clean(pressure: f32) -> f32 {
    if pressure.is_finite() {
        pressure.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// `max(min_radius, base * pressure)`, never above `base`. Full pressure
/// returns `base` untouched (bit for bit).
#[must_use]
pub(crate) fn pressured_radius(base: f32, pressure: f32) -> f32 {
    let p = clean(pressure);
    if p >= 1.0 {
        return base;
    }
    (base * p).max(MIN_PRESSURE_RADIUS_PX.min(base))
}

/// `amount` with its Opacity ceiling scaled by `pressure`. Full pressure
/// returns `amount` untouched.
#[must_use]
pub(crate) fn pressured_amount(amount: PaintAmount, pressure: f32) -> PaintAmount {
    let p = clean(pressure);
    if p >= 1.0 {
        return amount;
    }
    PaintAmount::new(amount.opacity() * p, amount.flow())
}

/// The pressure at `point`, a dab on the segment `from -> to`, given the
/// pressure `p_from` at `from` and `p_to` at `to`: linear in the dab's
/// distance from `from` over the segment's length. A zero-length segment
/// has `p_to`.
#[must_use]
pub(crate) fn pressure_along(
    from: (f32, f32),
    to: (f32, f32),
    p_from: f32,
    p_to: f32,
    point: (f32, f32),
) -> f32 {
    let (p_from, p_to) = (clean(p_from), clean(p_to));
    let len = (to.0 - from.0).hypot(to.1 - from.1);
    if len.is_nan() || len <= 0.0 {
        return p_to;
    }
    let t = ((point.0 - from.0).hypot(point.1 - from.1) / len).clamp(0.0, 1.0);
    p_from + (p_to - p_from) * t
}

/// The spacing step for a segment whose end pressures are `p_from` and
/// `p_to`: `aurora_brush::dab_step` of the pressured radius at their mean,
/// or of the base radius when pressure does not control size.
#[must_use]
pub(crate) fn segment_step(base_radius: f32, size_on: bool, p_from: f32, p_to: f32) -> f32 {
    let radius = if size_on {
        pressured_radius(base_radius, 0.5 * (clean(p_from) + clean(p_to)))
    } else {
        base_radius
    };
    aurora_brush::dab_step(radius, aurora_brush::DEFAULT_SPACING)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;

    #[test]
    fn full_pressure_is_the_base_radius_bit_for_bit() {
        for base in [0.5_f32, 1.0, 7.3, 24.0, 256.0] {
            assert_eq!(pressured_radius(base, 1.0).to_bits(), base.to_bits());
        }
        assert_eq!(pressured_radius(24.0, f32::NAN), 24.0);
        assert_eq!(pressured_radius(24.0, 7.0), 24.0);
    }

    #[test]
    fn radius_scales_with_pressure_and_floors_at_one_pixel() {
        assert_eq!(pressured_radius(20.0, 0.5), 10.0);
        assert_eq!(pressured_radius(20.0, 0.0), 1.0);
        assert_eq!(pressured_radius(20.0, 0.01), 1.0);
        assert_eq!(pressured_radius(20.0, -3.0), 1.0);
        // The floor never exceeds the base.
        assert_eq!(pressured_radius(1.0, 0.2), 1.0);
        assert_eq!(pressured_radius(0.5, 0.2), 0.5);
    }

    #[test]
    fn amount_ceiling_scales_with_pressure_and_flow_is_kept() {
        let base = PaintAmount::new(0.8, 0.5);
        assert_eq!(pressured_amount(base, 1.0), base);
        let half = pressured_amount(base, 0.5);
        assert!((half.opacity() - 0.4).abs() < 1e-6);
        assert_eq!(half.flow(), 0.5);
        // Never below the engine's floor.
        assert_eq!(
            pressured_amount(base, 0.0).opacity(),
            aurora_brush::MIN_AMOUNT
        );
    }

    #[test]
    fn pressure_interpolates_linearly_along_a_segment() {
        let (a, b) = ((0.0, 0.0), (10.0, 0.0));
        assert_eq!(pressure_along(a, b, 0.2, 1.0, a), 0.2);
        assert_eq!(pressure_along(a, b, 0.2, 1.0, b), 1.0);
        assert!((pressure_along(a, b, 0.2, 1.0, (5.0, 0.0)) - 0.6).abs() < 1e-6);
        // Diagonal segment: by distance, not by x alone.
        let d = pressure_along((0.0, 0.0), (3.0, 4.0), 0.0, 1.0, (1.5, 2.0));
        assert!((d - 0.5).abs() < 1e-6);
        // A zero-length segment is the end pressure; a point past the end clamps.
        assert_eq!(pressure_along(a, a, 0.2, 0.9, a), 0.9);
        assert_eq!(pressure_along(a, b, 0.2, 1.0, (50.0, 0.0)), 1.0);
    }

    #[test]
    fn spacing_follows_the_pressured_radius_with_the_engine_floor() {
        let full = aurora_brush::dab_step(40.0, aurora_brush::DEFAULT_SPACING);
        assert_eq!(segment_step(40.0, true, 1.0, 1.0), full);
        assert_eq!(segment_step(40.0, false, 0.1, 0.1), full);
        assert!(segment_step(40.0, true, 0.5, 0.5) < full);
        // Tiny pressure cannot go below the engine's 0.5 px step.
        assert_eq!(segment_step(40.0, true, 0.0, 0.0), 0.5);
    }
}
