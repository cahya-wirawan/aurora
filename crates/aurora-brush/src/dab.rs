//! Stroke input → dab positions. PLAN.md M1.9's "basic brush and eraser"
//! bullet (a *basic* one — the real engine is Phase 2, per that bullet's
//! own name) — this crate's first real code.
//!
//! **Scope, stated honestly.** This is exactly the "dab scheduling" half
//! of this crate's own doc comment (stroke input, stabilization, and dab
//! scheduling): turning a path of pointer positions into evenly spaced
//! dab centers, the same spacing formula (`step = radius * spacing`,
//! floored at `0.5`) `spike/vertical-slice::doc::stroke_segment` already
//! measured and found workable (`spike/FINDINGS.md`: p99 stroke latency
//! 9.1 ms against a 10 ms budget). [`dabs_along_path`] generalizes that
//! spike's own single-segment version to a full multi-point path,
//! carrying leftover spacing distance across segment boundaries — a
//! stroke built from many short segments (a high-polling-rate pointer)
//! must not double up dabs at every point the way naively restarting the
//! spacing countdown at each segment would.
//!
//! This module is pure geometry — no pixel storage involved at all.
//! [`crate::stamp`] is where a dab position actually becomes pixels
//! (`aurora_tile::TileStore`, addressed by `SurfaceId` per [ADR
//! 0010](../../../docs/adr/0010-layer-pixel-storage.md)); this module
//! only produces the positions `stamp::stamp_dab` consumes.

/// Dab spacing as a fraction of brush radius — `spike/vertical-slice`'s
/// own proven default (a dab every quarter-radius travelled).
pub const DEFAULT_SPACING: f32 = 0.25;

/// The distance between dabs — `radius * spacing`, floored at `0.5` so a
/// degenerate `spacing` of `0.0` can't demand a dab at every
/// float-epsilon step. Shared by [`dabs_along_path`] and
/// [`advance_segment`] so both use the exact same formula.
#[must_use]
pub fn dab_step(radius: f32, spacing: f32) -> f32 {
    (radius * spacing).max(0.5)
}

/// Advances dab spacing across one segment from `prev` to `point`, given
/// `carry` (how far the path had already travelled past the last placed
/// dab, before this segment started) and `step` ([`dab_step`]). Returns
/// the dabs this segment placed (possibly none, for a segment shorter
/// than what's left before the next one) and the updated carry to feed
/// into the *next* segment's own call.
///
/// This is [`dabs_along_path`]'s own per-segment engine, exposed so a
/// caller driving a live pointer drag one move-event at a time (not a
/// fully-known path up front) can carry spacing correctly across many
/// small events — calling [`dabs_along_path`] fresh on each two-point
/// event would restart `carry` at `0.0` every time, which silently loses
/// spacing across events shorter than one `step` (a slow drag could
/// place no dabs at all past the first, despite covering real distance
/// over several events).
///
/// A zero-length segment (`prev == point`, e.g. two pointer-move events
/// with no actual movement) places nothing and returns `carry`
/// unchanged.
#[must_use]
pub fn advance_segment(
    prev: (f32, f32),
    point: (f32, f32),
    carry: f32,
    step: f32,
) -> (Vec<(f32, f32)>, f32) {
    let (dx, dy) = (point.0 - prev.0, point.1 - prev.1);
    let segment_len = dx.hypot(dy);
    if segment_len <= f32::EPSILON {
        return (Vec::new(), carry);
    }
    let (ux, uy) = (dx / segment_len, dy / segment_len);
    let mut dabs = Vec::new();
    let mut travelled = 0.0_f32;
    let mut carry = carry;
    while carry + (segment_len - travelled) >= step {
        let needed = step - carry;
        travelled += needed;
        dabs.push((prev.0 + ux * travelled, prev.1 + uy * travelled));
        carry = 0.0;
    }
    carry += segment_len - travelled;
    (dabs, carry)
}

/// Generates evenly spaced dab centers along the polyline through
/// `points`, [`dab_step`] apart.
///
/// Always includes `points`'s own first point (a stroke's very first dab
/// lands exactly where the pointer went down, not one `step` later).
///
/// Spacing carries across segment boundaries ([`advance_segment`]):
/// splitting one straight path into many short segments (any way the
/// points get chopped up, as long as they still lie on it) produces the
/// exact same dab positions as one long segment — see this function's
/// own tests.
#[must_use]
pub fn dabs_along_path(points: &[(f32, f32)], radius: f32, spacing: f32) -> Vec<(f32, f32)> {
    let mut dabs = Vec::new();
    let mut iter = points.iter().copied();
    let Some(first) = iter.next() else {
        return dabs;
    };
    dabs.push(first);

    let step = dab_step(radius, spacing);
    let mut prev = first;
    let mut carry = 0.0_f32;

    for point in iter {
        let (segment_dabs, new_carry) = advance_segment(prev, point, carry, step);
        dabs.extend(segment_dabs);
        carry = new_carry;
        prev = point;
    }
    dabs
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_SPACING, advance_segment, dab_step, dabs_along_path};

    #[test]
    fn empty_path_produces_no_dabs() {
        assert_eq!(dabs_along_path(&[], 10.0, DEFAULT_SPACING), Vec::new());
    }

    #[test]
    fn a_single_point_produces_exactly_that_one_dab() {
        assert_eq!(
            dabs_along_path(&[(5.0, 7.0)], 10.0, DEFAULT_SPACING),
            vec![(5.0, 7.0)]
        );
    }

    #[test]
    fn a_straight_segment_places_dabs_at_the_expected_step() {
        // radius 10, spacing 0.25 -> step 2.5; a 10-unit horizontal
        // segment should land dabs at 0, 2.5, 5, 7.5, 10.
        let dabs = dabs_along_path(&[(0.0, 0.0), (10.0, 0.0)], 10.0, DEFAULT_SPACING);
        assert_eq!(
            dabs,
            vec![(0.0, 0.0), (2.5, 0.0), (5.0, 0.0), (7.5, 0.0), (10.0, 0.0),]
        );
    }

    #[test]
    fn spacing_is_floored_at_half_a_unit() {
        // radius 1, spacing 0.0 -> a literal step of 0.0 would place
        // infinitely many dabs; the 0.5 floor keeps this finite.
        let dabs = dabs_along_path(&[(0.0, 0.0), (2.0, 0.0)], 1.0, 0.0);
        assert_eq!(
            dabs,
            vec![(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (1.5, 0.0), (2.0, 0.0)]
        );
    }

    #[test]
    fn zero_length_segments_are_skipped_not_treated_as_a_direction() {
        let dabs = dabs_along_path(
            &[(0.0, 0.0), (0.0, 0.0), (10.0, 0.0)],
            10.0,
            DEFAULT_SPACING,
        );
        assert_eq!(
            dabs,
            vec![(0.0, 0.0), (2.5, 0.0), (5.0, 0.0), (7.5, 0.0), (10.0, 0.0),]
        );
    }

    #[test]
    fn spacing_carries_across_segment_boundaries() {
        // The same 10-unit path, once as a single segment and once
        // chopped into three uneven segments -- must produce identical
        // dabs either way, proving the carry-over logic (not just
        // "restart the spacing countdown at every point") is correct.
        let one_segment = dabs_along_path(&[(0.0, 0.0), (10.0, 0.0)], 10.0, DEFAULT_SPACING);
        let three_segments = dabs_along_path(
            &[(0.0, 0.0), (1.0, 0.0), (6.0, 0.0), (10.0, 0.0)],
            10.0,
            DEFAULT_SPACING,
        );
        assert_eq!(one_segment, three_segments);
    }

    #[test]
    fn works_on_a_diagonal_path_too() {
        // radius 10, spacing 0.25 -> step 2.5; a diagonal segment of
        // length 5 (3-4-5 triangle) should land a dab exactly halfway.
        let dabs = dabs_along_path(&[(0.0, 0.0), (3.0, 4.0)], 10.0, DEFAULT_SPACING);
        assert_eq!(dabs.len(), 3);
        let Some(&midpoint) = dabs.get(1) else {
            unreachable!("just asserted len() == 3");
        };
        assert!((midpoint.0 - 1.5).abs() < 1e-4, "{midpoint:?}");
        assert!((midpoint.1 - 2.0).abs() < 1e-4, "{midpoint:?}");
    }

    #[test]
    // `dab_step` is one multiplication (or the fixed `0.5` floor), never
    // accumulated -- exact equality is correct here.
    #[allow(clippy::float_cmp)]
    fn dab_step_matches_the_documented_formula() {
        assert_eq!(dab_step(10.0, 0.25), 2.5);
        assert_eq!(dab_step(1.0, 0.0), 0.5, "must floor at 0.5");
    }

    #[test]
    // `carry` here is `0.0` because the loop's own exact arithmetic
    // lands exactly on the segment's end (4 * 2.5 == 10.0), not because
    // of accumulated rounding -- exact equality is correct.
    #[allow(clippy::float_cmp)]
    fn advance_segment_with_zero_carry_matches_dabs_along_path() {
        let step = dab_step(10.0, DEFAULT_SPACING);
        let (dabs, carry) = advance_segment((0.0, 0.0), (10.0, 0.0), 0.0, step);
        assert_eq!(dabs, vec![(2.5, 0.0), (5.0, 0.0), (7.5, 0.0), (10.0, 0.0)]);
        assert_eq!(
            carry, 0.0,
            "landing exactly on the segment's end leaves nothing to carry"
        );
    }

    #[test]
    // `carry` is exactly the 1-unit segment length itself (no dab
    // placed, so no further arithmetic touched it) -- exact equality is
    // correct.
    #[allow(clippy::float_cmp)]
    fn advance_segment_carries_a_short_segment_forward() {
        let step = dab_step(10.0, DEFAULT_SPACING);
        // A 1-unit segment is shorter than the 2.5-unit step -- no dab
        // yet, but the 1 unit already travelled must carry forward.
        let (dabs, carry) = advance_segment((0.0, 0.0), (1.0, 0.0), 0.0, step);
        assert_eq!(dabs, Vec::new());
        assert_eq!(carry, 1.0);
    }

    #[test]
    fn advance_segment_called_event_by_event_matches_one_whole_call() {
        // The same 10-unit path, once as a single `advance_segment` call
        // and once as several small ones each carrying `carry` forward --
        // must produce the same dabs in total, proving this function
        // (not just `dabs_along_path`'s own internal use of it) carries
        // spacing correctly across many small steps, the exact scenario
        // a live pointer drag's many move events puts it in.
        let step = dab_step(10.0, DEFAULT_SPACING);
        let (whole, _) = advance_segment((0.0, 0.0), (10.0, 0.0), 0.0, step);

        let mut piecewise = Vec::new();
        let mut carry = 0.0;
        let mut prev = (0.0, 0.0);
        for next in [(1.0, 0.0), (2.0, 0.0), (3.0, 0.0), (10.0, 0.0)] {
            let (dabs, new_carry) = advance_segment(prev, next, carry, step);
            piecewise.extend(dabs);
            carry = new_carry;
            prev = next;
        }
        assert_eq!(whole, piecewise);
    }

    #[test]
    // `carry` is returned completely untouched for a zero-length
    // segment (an early return, no arithmetic at all) -- exact equality
    // is correct.
    #[allow(clippy::float_cmp)]
    fn advance_segment_of_a_zero_length_segment_carries_unchanged() {
        let (dabs, carry) = advance_segment((5.0, 5.0), (5.0, 5.0), 1.5, 2.5);
        assert_eq!(dabs, Vec::new());
        assert_eq!(carry, 1.5);
    }
}
