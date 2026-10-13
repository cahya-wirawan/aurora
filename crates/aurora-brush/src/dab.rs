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

/// The soft **Brush**'s dab spacing as a fraction of its radius (0.226.1):
/// a dab every `0.03 r` (floored at 0.5 px by [`dab_step`]), against the
/// [`DEFAULT_SPACING`] `0.25 r` every other dab tool keeps.
///
/// **Why the Brush has its own.** A Brush dab's alpha is `(1 - d / r)^1.5`
/// and a texel is rewritten only when a dab's alpha is *higher* than what
/// it already holds (the max-alpha rule, `stamp.rs`), so along a stroke the
/// alpha on the centre line is the upper envelope of the dabs. Halfway
/// between two dabs `s` apart the nearest dabs are `s / 2` away, so the
/// centre line dips from `1` to `(1 - s / (2 r))^1.5` there, a dip of
/// [`spacing_dip`]`(s / r) ~= 0.75 s / r`. At `0.25` that is **18 %** (the
/// beading the design owner reported: dark dots at each dab centre and
/// lighter bands between). At `0.03` it is **2.2 %**.
///
/// | radius | step at `0.03` | dip |
/// |---|---|---|
/// | 4 px | 0.5 px (the floor) | 9.1 % |
/// | 10 px | 0.5 px (the floor) | 3.7 % |
/// | 24 px | 0.72 px | 2.2 % |
/// | 100 px | 3.0 px | 2.2 % |
/// | 256 px | 7.68 px | 2.2 % |
///
/// Below about `r = 17` the 0.5 px floor, not this fraction, sets the step,
/// so the smallest brushes keep a larger dip (still half the old one).
///
/// **Cost.** Dense dabs cost proportionally more: measured in the dev
/// profile a dab is about 80 us at `r = 24` and 1.2 ms at `r = 100`, so a
/// 200 px stroke is 278 dabs (22 ms) at `r = 24` and 67 dabs (78 ms) at
/// `r = 100`, against 34 (3 ms) and 9 (14 ms) at `0.25`. A single pointer
/// event is bounded by [`brush_dab_cap`] instead of being allowed to take
/// that long.
///
/// Only the Brush uses this: every other tool's per-dab effect accumulates
/// (the Eraser multiplies, the smudge, blur, sharpen and toning tools mix
/// per dab, the Quick Mask moves toward its target per dab), so denser
/// dabs would change their strength, not just smooth them.
pub const BRUSH_SPACING: f32 = 0.03;

/// The most texel visits a Brush stroke is allowed per pointer event,
/// `250,000`: about 10 ms at the measured ~40 ns per texel (dev profile),
/// the stroke budget. [`brush_dab_cap`] turns it into a dab count for a
/// radius.
pub const BRUSH_EVENT_TEXEL_BUDGET: f32 = 250_000.0;

/// The fewest dabs per pointer event [`brush_dab_cap`] allows, however big
/// the radius (a 256 px dab is 205,000 texels, so the budget alone would
/// allow one).
pub const MIN_BRUSH_DABS_PER_EVENT: usize = 4;

/// The most dabs per pointer event [`brush_dab_cap`] allows, however small
/// the radius.
pub const MAX_BRUSH_DABS_PER_EVENT: usize = 256;

/// The depth of the dip a stroke at `spacing` (a fraction of the radius)
/// leaves in the Brush's centre-line alpha, as a fraction of full alpha:
/// `1 - (1 - spacing / 2)^1.5`, from the falloff `(1 - d / r)^1.5` read at
/// the midpoint between two dabs. `0` for a non-positive or non-finite
/// spacing; a spacing of `2` or more leaves the dabs disjoint, a full dip
/// of `1`.
#[must_use]
pub fn spacing_dip(spacing: f32) -> f32 {
    if !(spacing.is_finite() && spacing > 0.0) {
        return 0.0;
    }
    1.0 - (1.0 - spacing / 2.0).clamp(0.0, 1.0).powf(1.5)
}

/// How many dabs one Brush pointer event may place at `radius`:
/// [`BRUSH_EVENT_TEXEL_BUDGET`] divided by the dab's area `pi r^2`, rounded
/// down and kept within [`MIN_BRUSH_DABS_PER_EVENT`] and
/// [`MAX_BRUSH_DABS_PER_EVENT`] (138 at `r = 24`, 7 at `r = 100`, 4 at
/// `r = 256`). A non-finite or non-positive radius gets the maximum (it
/// paints nothing).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn brush_dab_cap(radius: f32) -> usize {
    let area = std::f32::consts::PI * radius * radius;
    if !(area.is_finite() && area > 0.0) {
        return MAX_BRUSH_DABS_PER_EVENT;
    }
    let cap = (BRUSH_EVENT_TEXEL_BUDGET / area).floor();
    // Within the usize range after the clamp.
    cap.clamp(
        MIN_BRUSH_DABS_PER_EVENT as f32,
        MAX_BRUSH_DABS_PER_EVENT as f32,
    ) as usize
}

/// The step the Brush uses for one pointer event of `segment_len` px, given
/// the `carry` the stroke had already travelled past its last dab:
/// [`dab_step`]`(radius, `[`BRUSH_SPACING`]`)`, widened to
/// `(carry + segment_len) / `[`brush_dab_cap`] when the dense step would put
/// more than the cap on this one event, so the event places **at most**
/// [`brush_dab_cap`] dabs. The carry stays in distance units, so the next
/// event (with its own step) continues from where this one stopped; a
/// stroke fed in events short enough never to reach the cap gets exactly
/// the dabs one long segment would.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn brush_segment_step(radius: f32, carry: f32, segment_len: f32) -> f32 {
    let dense = dab_step(radius, BRUSH_SPACING);
    let travel = carry.max(0.0) + segment_len.max(0.0);
    let capped = travel / brush_dab_cap(radius) as f32;
    if capped.is_finite() {
        dense.max(capped)
    } else {
        dense
    }
}

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
///
/// `step` may differ from one event to the next (0.226.1: the Brush widens
/// it for an event that would otherwise place too many dabs,
/// [`brush_segment_step`]); `carry` is a distance, so it stays meaningful.
/// A `carry` already at or past the new `step` puts a dab at the start of
/// the segment.
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
        // `carry` is below `step` unless the step shrank since the last
        // event (the Brush's capped flick followed by dense events,
        // `brush_segment_step`): then a dab is already due and lands at the
        // segment's start, never behind it.
        let needed = (step - carry).max(0.0);
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
