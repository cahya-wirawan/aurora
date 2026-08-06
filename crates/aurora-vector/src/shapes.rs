//! Common UI shapes, expressed as [`crate::Path`]s.

use crate::path::{Path, PathBuilder};
use crate::point::Point;

/// The standard cubic-Bézier approximation of a quarter circle:
/// `4 * (sqrt(2) - 1) / 3`, the distance (as a fraction of the radius)
/// each corner's own control points sit from their endpoint along the
/// tangent direction. The same constant every mainstream 2D API's own
/// rounded-rect construction uses (SVG, Skia, CSS `border-radius`
/// rendering) — not a magic number invented here.
// Truncated to f32's own real precision (7ish significant digits) --
// the extra digits of the textbook constant are unrepresentable noise
// at this type, not a deliberately dropped one.
const KAPPA: f32 = 0.552_284_8;

/// A rectangle with all four corners rounded to `radius` — the single
/// most common UI shape (every button, panel, and field this project's
/// own design language uses one). `radius` is clamped to at most half
/// of whichever of `width`/`height` is smaller (a radius larger than
/// that has no more room to round into, the same clamp every mainstream
/// rounded-rect implementation applies) and to at least `0.0`; a `0.0`
/// radius degenerates into a plain rectangle (each "corner" becomes a
/// zero-length curve, which tessellates the same as a straight line).
#[must_use]
pub fn rounded_rect(x: f32, y: f32, width: f32, height: f32, radius: f32) -> Path {
    let r = radius.clamp(0.0, (width / 2.0).min(height / 2.0).max(0.0));
    let k = r * KAPPA;

    let mut builder = PathBuilder::new();
    builder
        .move_to(Point::new(x + r, y))
        .line_to(Point::new(x + width - r, y))
        .cubic_bezier_to(
            Point::new(x + width - r + k, y),
            Point::new(x + width, y + r - k),
            Point::new(x + width, y + r),
        )
        .line_to(Point::new(x + width, y + height - r))
        .cubic_bezier_to(
            Point::new(x + width, y + height - r + k),
            Point::new(x + width - r + k, y + height),
            Point::new(x + width - r, y + height),
        )
        .line_to(Point::new(x + r, y + height))
        .cubic_bezier_to(
            Point::new(x + r - k, y + height),
            Point::new(x, y + height - r + k),
            Point::new(x, y + height - r),
        )
        .line_to(Point::new(x, y + r))
        .cubic_bezier_to(
            Point::new(x, y + r - k),
            Point::new(x + r - k, y),
            Point::new(x + r, y),
        )
        .close();
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::rounded_rect;
    use crate::mesh::{DEFAULT_TOLERANCE, fill};

    #[test]
    fn a_zero_radius_rounded_rect_fills_to_the_same_area_as_a_plain_rectangle() {
        let path = rounded_rect(0.0, 0.0, 10.0, 20.0, 0.0);
        let mesh = match fill(&path, DEFAULT_TOLERANCE) {
            Ok(mesh) => mesh,
            Err(err) => unreachable!("{err}"),
        };
        let area = triangle_area_sum(&mesh);
        assert!(
            (area - 200.0).abs() < 0.5,
            "a 10x20 rect (area 200) with no rounding must tessellate to \
             very nearly its own full area, got {area}"
        );
    }

    #[test]
    fn rounding_the_corners_removes_real_area_versus_a_plain_rectangle() {
        let plain = fill(&rounded_rect(0.0, 0.0, 20.0, 20.0, 0.0), DEFAULT_TOLERANCE);
        let rounded = fill(&rounded_rect(0.0, 0.0, 20.0, 20.0, 6.0), DEFAULT_TOLERANCE);
        let (Ok(plain), Ok(rounded)) = (plain, rounded) else {
            unreachable!("both fills are well-formed closed shapes");
        };
        assert!(
            triangle_area_sum(&rounded) < triangle_area_sum(&plain),
            "rounded corners must cut real area out of the square"
        );
    }

    #[test]
    fn a_radius_larger_than_half_the_smaller_dimension_is_clamped_not_rejected() {
        // A radius far larger than the whole shape must not produce a
        // self-intersecting or negative-area path -- clamped to a
        // "fully rounded" pill/circle shape instead.
        let path = rounded_rect(0.0, 0.0, 10.0, 4.0, 1000.0);
        if let Err(err) = fill(&path, DEFAULT_TOLERANCE) {
            unreachable!("an oversized radius must still tessellate cleanly: {err}");
        }
    }

    /// The sum of every triangle's own area in `mesh` — the shoelace
    /// formula per triangle, summed. What both tests above use to
    /// confirm real geometric area, not just "some vertices exist."
    fn triangle_area_sum(mesh: &crate::mesh::Mesh) -> f32 {
        let mut total = 0.0;
        for triangle in mesh.indices.chunks_exact(3) {
            let &[i0, i1, i2] = triangle else {
                unreachable!("chunks_exact(3) always yields length-3 slices");
            };
            let (Some(a), Some(b), Some(c)) = (
                mesh.vertices.get(i0 as usize),
                mesh.vertices.get(i1 as usize),
                mesh.vertices.get(i2 as usize),
            ) else {
                unreachable!("every index came from this same mesh's own vertices");
            };
            total += ((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)).abs() / 2.0;
        }
        total
    }
}
