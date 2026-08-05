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
//! only produces the positions `stamp::stamp_stroke` consumes.

/// Dab spacing as a fraction of brush radius — `spike/vertical-slice`'s
/// own proven default (a dab every quarter-radius travelled).
pub const DEFAULT_SPACING: f32 = 0.25;

/// Generates evenly spaced dab centers along the polyline through
/// `points`, `radius * spacing` apart (floored at `0.5`, so a degenerate
/// `spacing` of `0.0` can't produce a dab at every float-epsilon step).
///
/// Always includes `points`'s own first point (a stroke's very first dab
/// lands exactly where the pointer went down, not one `step` later).
/// Consecutive duplicate points (a zero-length segment — e.g. two
/// pointer-move events with no actual movement) are skipped rather than
/// treated as a stroke direction, since a zero-length segment has none.
///
/// Spacing carries across segment boundaries: splitting one straight
/// path into many short segments (any way the points get chopped up, as
/// long as they still lie on it) produces the exact same dab positions
/// as one long segment — see this function's own tests.
#[must_use]
pub fn dabs_along_path(points: &[(f32, f32)], radius: f32, spacing: f32) -> Vec<(f32, f32)> {
    let mut dabs = Vec::new();
    let mut iter = points.iter().copied();
    let Some(first) = iter.next() else {
        return dabs;
    };
    dabs.push(first);

    let step = (radius * spacing).max(0.5);
    let mut prev = first;
    // How far the path has travelled since the last dab was placed --
    // persists across segments so spacing is continuous along the whole
    // path, not reset to zero at every point.
    let mut distance_since_last_dab = 0.0_f32;

    for point in iter {
        let (dx, dy) = (point.0 - prev.0, point.1 - prev.1);
        let segment_len = dx.hypot(dy);
        if segment_len <= f32::EPSILON {
            prev = point;
            continue;
        }
        let (ux, uy) = (dx / segment_len, dy / segment_len);
        let mut travelled = 0.0_f32;
        while distance_since_last_dab + (segment_len - travelled) >= step {
            let needed = step - distance_since_last_dab;
            travelled += needed;
            dabs.push((prev.0 + ux * travelled, prev.1 + uy * travelled));
            distance_since_last_dab = 0.0;
        }
        distance_since_last_dab += segment_len - travelled;
        prev = point;
    }
    dabs
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_SPACING, dabs_along_path};

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
}
